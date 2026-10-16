use crate::account_session::AccountSession;
use anyhow::Result;
use chrono::{DateTime, Utc};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::{Attribute, Print, SetAttribute},
    terminal,
};
use rymnet_board::{Comment, Post, PostSummary};
use rymnet_network::{PeerMessage, send_request};
use rymnet_storage::{DataLayout, PeerConfig};
use serde_json::json;
use std::collections::HashSet;
use std::io::{Write, stdout};
use std::time::{Duration, Instant};

/// Same relay-and-poll shape as `irc.rs` -- no presence lookup, just
/// broadcast-to-configured-peers on send and poll-configured-peers on
/// read, deduped by id. Near-real-time, not true push.
const POLL_INTERVAL: Duration = Duration::from_secs(3);

enum BoardView {
    List,
    Thread { post: Box<Post>, comments: Vec<Comment> },
}

struct BoardState {
    board: Option<String>,
    posts: Vec<PostSummary>,
    selected: usize,
    view: BoardView,
    input: String,
    status: Option<String>,
    last_poll: Instant,
}

pub async fn run(layout: &DataLayout, session: &AccountSession) -> Result<()> {
    let peers = PeerConfig::load(layout.peers_path())?;
    if peers.peers.is_empty() {
        anyhow::bail!("no peers configured; add one with `peer add` before joining a board");
    }

    let mut state = BoardState {
        board: None,
        posts: Vec::new(),
        selected: 0,
        view: BoardView::List,
        input: String::new(),
        status: Some("Type /board <name> to start, or /help for commands.".to_string()),
        last_poll: Instant::now() - POLL_INTERVAL,
    };

    terminal::enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, terminal::EnterAlternateScreen, cursor::Hide)?;

    let result = run_loop(&mut out, layout, session, &peers, &mut state).await;

    execute!(out, cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;
    result
}

async fn run_loop(
    out: &mut impl Write,
    layout: &DataLayout,
    session: &AccountSession,
    peers: &PeerConfig,
    state: &mut BoardState,
) -> Result<()> {
    loop {
        draw(out, state)?;

        if state.board.is_some() && matches!(state.view, BoardView::List) && state.last_poll.elapsed() >= POLL_INTERVAL {
            poll_board(layout, session, peers, state).await;
            state.last_poll = Instant::now();
            continue;
        }

        if event::poll(Duration::from_millis(250))? {
            let Event::Key(KeyEvent { code, modifiers, .. }) = event::read()? else {
                continue;
            };
            match (code, modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                (KeyCode::Up, _) if matches!(state.view, BoardView::List) => {
                    state.selected = state.selected.saturating_sub(1);
                }
                (KeyCode::Down, _) if matches!(state.view, BoardView::List) && state.selected + 1 < state.posts.len() => {
                    state.selected += 1;
                }
                (KeyCode::Esc, _) if !matches!(state.view, BoardView::List) => {
                    state.view = BoardView::List;
                    state.status = None;
                }
                (KeyCode::Enter, _) => {
                    let line = std::mem::take(&mut state.input);
                    if handle_line(session, peers, state, &line).await? {
                        break;
                    }
                }
                (KeyCode::Backspace, _) => {
                    state.input.pop();
                }
                (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                    state.input.push(ch);
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Returns `true` if the REPL should exit.
async fn handle_line(session: &AccountSession, peers: &PeerConfig, state: &mut BoardState, line: &str) -> Result<bool> {
    let line = line.trim();

    if let Some(rest) = line.strip_prefix("/board ") {
        let board = rest.trim().to_string();
        if let Err(err) = rymnet_board::validate_board(&board) {
            state.status = Some(format!("{err:#}"));
            return Ok(false);
        }
        state.board = Some(board);
        state.posts.clear();
        state.selected = 0;
        state.view = BoardView::List;
        state.status = Some("Joining...".to_string());
        state.last_poll = Instant::now() - POLL_INTERVAL;
        return Ok(false);
    }

    if let Some(rest) = line.strip_prefix("/new ") {
        let Some(board) = state.board.clone() else {
            state.status = Some("not on a board -- /board <name> first".to_string());
            return Ok(false);
        };
        let mut parts = rest.splitn(2, '|');
        let (Some(title), Some(body)) = (parts.next(), parts.next()) else {
            state.status = Some("usage: /new <title> | <body>".to_string());
            return Ok(false);
        };
        match Post::create(&board, &session.username, &session.identity, title.trim(), body.trim(), None) {
            Ok(post) => {
                let mut delivered = 0;
                for peer in &peers.peers {
                    if send_request(peer, &session.identity, PeerMessage::new("board.post.deliver", json!({ "post": &post })))
                        .await
                        .is_ok_and(|response| response.payload.get("status").and_then(|v| v.as_str()) == Some("accepted"))
                    {
                        delivered += 1;
                    }
                }
                // Echo locally right away rather than waiting for the
                // next poll to see our own post.
                state.posts.push(post.summary());
                state.status = Some(format!("posted (delivered to {delivered}/{} peer(s))", peers.peers.len()));
            }
            Err(err) => {
                state.status = Some(format!("post failed: {err:#}"));
            }
        }
        return Ok(false);
    }

    if line == "/back" {
        state.view = BoardView::List;
        state.status = None;
        return Ok(false);
    }

    if line == "/quit" {
        return Ok(true);
    }

    if line == "/help" {
        state.status = Some(
            "/board <name>   /new <title> | <body>   /back   /quit   Ctrl-C to leave -- \
             Up/Down selects a post, Enter opens it; inside a thread, plain text posts a comment."
                .to_string(),
        );
        return Ok(false);
    }

    if line.starts_with('/') {
        state.status = Some(format!("unknown command: {line} (try /help)"));
        return Ok(false);
    }

    // Whether we're currently inside a thread, extracted as owned data
    // before any further borrow of `state` -- avoids holding a borrow of
    // `state.view` across the mutation below.
    let thread_target: Option<(String, String)> = match &state.view {
        BoardView::Thread { post, .. } => Some((post.board.clone(), post.id.clone())),
        BoardView::List => None,
    };

    match thread_target {
        None => {
            if line.is_empty() {
                open_selected(session, peers, state).await;
            } else {
                state.status = Some("select a post with Up/Down and press Enter, or use /board /new /quit /help".to_string());
            }
        }
        Some((board, post_id)) if !line.is_empty() => {
            match Comment::create(&board, &post_id, &session.username, &session.identity, line) {
                Ok(comment) => {
                    let mut delivered = 0;
                    for peer in &peers.peers {
                        if send_request(peer, &session.identity, PeerMessage::new("board.comment.deliver", json!({ "comment": &comment })))
                            .await
                            .is_ok_and(|response| response.payload.get("status").and_then(|v| v.as_str()) == Some("accepted"))
                        {
                            delivered += 1;
                        }
                    }
                    if let BoardView::Thread { comments, .. } = &mut state.view {
                        comments.push(comment);
                    }
                    state.status = Some(format!("commented (delivered to {delivered}/{} peer(s))", peers.peers.len()));
                }
                Err(err) => {
                    state.status = Some(format!("comment failed: {err:#}"));
                }
            }
        }
        Some(_) => {}
    }
    Ok(false)
}

/// Fetches the selected post in full plus its comments from every
/// configured peer (deduped by id, each independently signature-checked
/// -- comment volume per post is human-scale, so unlike `poll_board`
/// there's no lightweight-index shortcut needed here) and switches the
/// view to it.
async fn open_selected(session: &AccountSession, peers: &PeerConfig, state: &mut BoardState) {
    let Some(board) = state.board.clone() else { return };
    let Some(summary) = state.posts.get(state.selected).cloned() else {
        state.status = Some("no posts to open".to_string());
        return;
    };
    state.status = Some("Opening...".to_string());

    let mut post = None;
    for peer in &peers.peers {
        if post.is_some() {
            break;
        }
        let payload = json!({ "board": board, "post_id": summary.id });
        if let Ok(response) = send_request(peer, &session.identity, PeerMessage::new("board.post.get", payload)).await
            && let Some(post_value) = response.payload.get("post").filter(|v| !v.is_null())
            && let Ok(fetched) = serde_json::from_value::<Post>(post_value.clone())
            && fetched.verify_signature().unwrap_or(false)
        {
            post = Some(fetched);
        }
    }
    let Some(post) = post else {
        state.status = Some("could not fetch that post from any peer".to_string());
        return;
    };

    let mut seen_ids = HashSet::new();
    let mut comments: Vec<Comment> = Vec::new();
    for peer in &peers.peers {
        let payload = json!({ "board": board, "post_id": post.id });
        if let Ok(response) = send_request(peer, &session.identity, PeerMessage::new("board.comment.fetch", payload)).await
            && let Some(values) = response.payload.get("comments").and_then(|v| v.as_array())
        {
            for value in values {
                if let Ok(comment) = serde_json::from_value::<Comment>(value.clone())
                    && comment.verify_signature().unwrap_or(false)
                    && seen_ids.insert(comment.id.clone())
                {
                    comments.push(comment);
                }
            }
        }
    }
    comments.sort_by_key(|comment| comment.created_at);

    state.view = BoardView::Thread { post: Box::new(post), comments };
    state.status = None;
}

/// Local disk first (the board's append-only index, not a directory
/// scan of full post bodies -- see `PostSummary`'s doc comment for why
/// that distinction matters at thousands of posts), then every
/// configured peer, dedup by post id -- same ordering fix
/// mail/chat already needed twice.
async fn poll_board(layout: &DataLayout, session: &AccountSession, peers: &PeerConfig, state: &mut BoardState) {
    let Some(board) = state.board.clone() else { return };
    let Ok(dir_name) = rymnet_board::board_dir_name(&board) else { return };

    let mut seen_ids: HashSet<String> = state.posts.iter().map(|post| post.id.clone()).collect();

    let index_path = layout.board_index_path(&dir_name);
    if let Ok(text) = std::fs::read_to_string(&index_path) {
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(summary) = serde_json::from_str::<PostSummary>(line)
                && seen_ids.insert(summary.id.clone())
            {
                state.posts.push(summary);
            }
        }
    }

    for peer in &peers.peers {
        let since: Option<DateTime<Utc>> = state.posts.iter().map(|post| post.created_at).max();
        let mut payload = json!({ "board": board });
        if let Some(since) = since {
            payload["since"] = json!(since.to_rfc3339());
        }
        if let Ok(response) = send_request(peer, &session.identity, PeerMessage::new("board.post.fetch", payload)).await
            && let Some(values) = response.payload.get("posts").and_then(|v| v.as_array())
        {
            for value in values {
                if let Ok(summary) = serde_json::from_value::<PostSummary>(value.clone())
                    && seen_ids.insert(summary.id.clone())
                {
                    state.posts.push(summary);
                }
            }
        }
    }

    state.posts.sort_by_key(|post| post.created_at);
    if !state.posts.is_empty() && state.selected >= state.posts.len() {
        state.selected = state.posts.len() - 1;
    }
    state.status = None;
}

fn draw(out: &mut impl Write, state: &BoardState) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    queue!(out, terminal::Clear(terminal::ClearType::All))?;

    let header = match (&state.board, &state.view) {
        (Some(board), BoardView::List) => format!("rymboard -- {board}"),
        (Some(board), BoardView::Thread { post, .. }) => format!("rymboard -- {board} -- {}", post.title),
        (None, _) => "rymboard -- no board joined".to_string(),
    };
    queue!(out, cursor::MoveTo(0, 0), SetAttribute(Attribute::Reverse), Print(fit(&header, cols)), SetAttribute(Attribute::Reset))?;

    let body_height = rows.saturating_sub(4) as usize;
    match &state.view {
        BoardView::List => {
            if state.posts.is_empty() {
                queue!(out, cursor::MoveTo(0, 1), Print(fit("(no posts yet -- /new <title> | <body> to post)", cols)))?;
            } else {
                let window_start = state
                    .selected
                    .saturating_sub(body_height.saturating_sub(1))
                    .min(state.posts.len().saturating_sub(body_height.max(1)));
                let visible = state.posts.iter().enumerate().skip(window_start).take(body_height);
                for (row, (index, post)) in (0_u16..).zip(visible) {
                    let marker = if index == state.selected { ">" } else { " " };
                    let link_marker = if post.has_link { " [link]" } else { "" };
                    let line = format!("{marker} {} by {}{}", post.title, post.from, link_marker);
                    queue!(out, cursor::MoveTo(0, row + 1), Print(fit(&line, cols)))?;
                }
            }
        }
        BoardView::Thread { post, comments } => {
            let mut row: u16 = 0;
            queue!(
                out,
                cursor::MoveTo(0, row + 1),
                Print(fit(&format!("by {} -- {}", post.from, post.created_at.format("%Y-%m-%d %H:%M")), cols))
            )?;
            row += 1;
            if let Some(url) = &post.link_url {
                queue!(out, cursor::MoveTo(0, row + 1), Print(fit(&format!("link: {url}"), cols)))?;
                row += 1;
            }
            for line in post.body.lines() {
                if row as usize >= body_height {
                    break;
                }
                queue!(out, cursor::MoveTo(0, row + 1), Print(fit(line, cols)))?;
                row += 1;
            }
            if (row as usize) < body_height {
                queue!(out, cursor::MoveTo(0, row + 1), Print(fit(&format!("-- {} comment(s) --", comments.len()), cols)))?;
                row += 1;
            }
            for comment in comments {
                if row as usize >= body_height {
                    break;
                }
                let line = format!("[{}] {}: {}", comment.created_at.format("%H:%M"), comment.from, comment.body);
                queue!(out, cursor::MoveTo(0, row + 1), Print(fit(&line, cols)))?;
                row += 1;
            }
        }
    }

    if let Some(status) = &state.status {
        queue!(out, cursor::MoveTo(0, rows.saturating_sub(2)), SetAttribute(Attribute::Reverse), Print(fit(status, cols)), SetAttribute(Attribute::Reset))?;
    }

    let prompt = format!("> {}", state.input);
    queue!(out, cursor::MoveTo(0, rows.saturating_sub(1)), Print(fit(&prompt, cols)))?;

    out.flush()?;
    Ok(())
}

fn fit(text: &str, width: u16) -> String {
    let width = width as usize;
    let mut out: String = text.chars().take(width).collect();
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}
