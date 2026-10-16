use crate::app::{AppContext, RymApp};
use crate::board;
use crate::theme;
use eframe::egui;
use rymnet_board::{Post, PostSummary};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Same reasoning as `ircapp.rs`'s own `POLL_INTERVAL`: without this,
/// there's no way for a new post from someone else to ever show up
/// short of manually re-joining the board.
const POLL_INTERVAL: Duration = Duration::from_secs(4);

type ListResult = (String, Result<Vec<PostSummary>, String>);
type ThreadResult = (String, String, Result<(Post, Vec<board::CommentView>), String>);

#[derive(Clone, Copy)]
enum PendingSend {
    Post,
    Comment,
}

type SendResult = (PendingSend, Result<(usize, usize), String>);

enum BoardView {
    List,
    Thread {
        board: String,
        post_id: String,
        // Boxed: `Post` carries several owned `String` fields, and
        // clippy's `large_enum_variant` flags the size gap against the
        // unit `List` variant otherwise.
        post: Option<Box<Post>>,
        comments: Vec<board::CommentView>,
        comment_input: String,
    },
}

struct ComposePost {
    title: String,
    body: String,
    link_url: String,
}

impl ComposePost {
    fn new() -> Self {
        Self { title: String::new(), body: String::new(), link_url: String::new() }
    }
}

/// The tenth desktop app: a simple, no-voting Reddit-style forum.
/// Boards on the left (mirrors `IrcApp`'s channel list), a post list or
/// an open thread in the center depending on `view`. Posts/comments are
/// signed but not encrypted -- same posture as chat -- so there's no
/// secret needed to read them; `AppContext.mailbox_secret` is untouched
/// here entirely.
pub struct BoardApp {
    boards: Vec<String>,
    active: usize,
    join_input: String,
    posts: std::collections::HashMap<String, Vec<PostSummary>>,
    view: BoardView,
    compose: Option<ComposePost>,
    list_rx: Option<mpsc::Receiver<ListResult>>,
    thread_rx: Option<mpsc::Receiver<ThreadResult>>,
    send_rx: Option<mpsc::Receiver<SendResult>>,
    status: Option<String>,
    status_is_error: bool,
    last_poll: Instant,
    first_frame: bool,
}

impl BoardApp {
    pub fn new() -> Self {
        Self {
            boards: Vec::new(),
            active: 0,
            join_input: String::new(),
            posts: std::collections::HashMap::new(),
            view: BoardView::List,
            compose: None,
            list_rx: None,
            thread_rx: None,
            send_rx: None,
            status: None,
            status_is_error: false,
            last_poll: Instant::now() - POLL_INTERVAL,
            first_frame: true,
        }
    }

    fn instance_id(&self) -> usize {
        self as *const Self as usize
    }

    fn active_board(&self) -> Option<String> {
        self.boards.get(self.active).cloned()
    }

    fn join_board(&mut self, app: &AppContext, board: String) {
        if let Err(err) = rymnet_board::validate_board(&board) {
            self.status = Some(format!("{err:#}"));
            self.status_is_error = true;
            return;
        }
        if let Some(idx) = self.boards.iter().position(|b| b == &board) {
            self.active = idx;
        } else {
            self.boards.push(board.clone());
            self.active = self.boards.len() - 1;
        }
        self.view = BoardView::List;
        self.last_poll = Instant::now() - POLL_INTERVAL;
        self.refresh_active(app);
    }

    fn refresh_active(&mut self, app: &AppContext) {
        let Some(board) = self.active_board() else { return };
        if self.list_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let layout = app.layout.clone();
        let identity = app.identity.clone();
        let board_for_task = board.clone();
        app.runtime.spawn(async move {
            let result = board::list_posts(&layout, &identity, &board_for_task).await.map_err(|err| format!("{err:#}"));
            let _ = tx.send((board_for_task, result));
        });
        self.list_rx = Some(rx);
    }

    fn poll_list(&mut self) {
        let Some(rx) = &self.list_rx else { return };
        let Ok((board, result)) = rx.try_recv() else { return };
        self.list_rx = None;
        match result {
            Ok(posts) => {
                self.posts.insert(board, posts);
            }
            Err(err) => {
                self.status = Some(format!("error: {err}"));
                self.status_is_error = true;
            }
        }
    }

    fn open_post(&mut self, app: &AppContext, board: String, post_id: String) {
        if self.thread_rx.is_some() {
            return;
        }
        self.view = BoardView::Thread { board: board.clone(), post_id: post_id.clone(), post: None, comments: Vec::new(), comment_input: String::new() };
        let (tx, rx) = mpsc::channel();
        let layout = app.layout.clone();
        let identity = app.identity.clone();
        let board_for_task = board.clone();
        let post_id_for_task = post_id.clone();
        app.runtime.spawn(async move {
            let result = board::open_thread(&layout, &identity, &board_for_task, &post_id_for_task).await.map_err(|err| format!("{err:#}"));
            let _ = tx.send((board_for_task, post_id_for_task, result));
        });
        self.thread_rx = Some(rx);
    }

    fn poll_thread(&mut self) {
        let Some(rx) = &self.thread_rx else { return };
        let Ok((board, post_id, result)) = rx.try_recv() else { return };
        self.thread_rx = None;
        match result {
            Ok((fetched_post, fetched_comments)) => {
                if let BoardView::Thread { board: current_board, post_id: current_post_id, post, comments, .. } = &mut self.view
                    && *current_board == board
                    && *current_post_id == post_id
                {
                    *post = Some(Box::new(fetched_post));
                    *comments = fetched_comments;
                }
            }
            Err(err) => {
                self.status = Some(format!("error: {err}"));
                self.status_is_error = true;
            }
        }
    }

    fn submit_post(&mut self, app: &AppContext, compose: ComposePost) {
        let Some(board) = self.active_board() else {
            self.status = Some("no board selected".to_string());
            self.status_is_error = true;
            return;
        };
        let (tx, rx) = mpsc::channel();
        let layout = app.layout.clone();
        let username = app.username.to_string();
        let identity = app.identity.clone();
        let link_url = if compose.link_url.trim().is_empty() { None } else { Some(compose.link_url.trim().to_string()) };
        let title = compose.title.trim().to_string();
        let body = compose.body.trim().to_string();
        app.runtime.spawn(async move {
            let result = board::submit_post(&layout, &username, &identity, &board, &title, &body, link_url).await.map_err(|err| format!("{err:#}"));
            let _ = tx.send((PendingSend::Post, result));
        });
        self.send_rx = Some(rx);
    }

    fn send_comment(&mut self, app: &AppContext) {
        if self.send_rx.is_some() {
            return;
        }
        let (board, post_id, text) = match &mut self.view {
            BoardView::Thread { board, post_id, comment_input, .. } => {
                let text = comment_input.trim().to_string();
                if text.is_empty() {
                    return;
                }
                comment_input.clear();
                (board.clone(), post_id.clone(), text)
            }
            BoardView::List => return,
        };
        let (tx, rx) = mpsc::channel();
        let layout = app.layout.clone();
        let username = app.username.to_string();
        let identity = app.identity.clone();
        app.runtime.spawn(async move {
            let result = board::submit_comment(&layout, &username, &identity, &board, &post_id, &text).await.map_err(|err| format!("{err:#}"));
            let _ = tx.send((PendingSend::Comment, result));
        });
        self.send_rx = Some(rx);
    }

    fn poll_send(&mut self, app: &AppContext) {
        let Some(rx) = &self.send_rx else { return };
        let Ok((kind, result)) = rx.try_recv() else { return };
        self.send_rx = None;
        match result {
            Ok((delivered, total)) => {
                self.status = Some(format!("sent (delivered to {delivered}/{total} peer(s))"));
                self.status_is_error = false;
                match kind {
                    PendingSend::Post => self.refresh_active(app),
                    PendingSend::Comment => {
                        let target = match &self.view {
                            BoardView::Thread { board, post_id, .. } => Some((board.clone(), post_id.clone())),
                            BoardView::List => None,
                        };
                        if let Some((board, post_id)) = target {
                            self.open_post(app, board, post_id);
                        }
                    }
                }
            }
            Err(err) => {
                self.status = Some(format!("send failed: {err}"));
                self.status_is_error = true;
            }
        }
    }

    fn board_list(&mut self, ui: &mut egui::Ui, app: &AppContext) {
        let mut switch_to = None;
        for (i, board) in self.boards.iter().enumerate() {
            if ui.selectable_label(self.active == i, board).clicked() {
                switch_to = Some(i);
            }
        }
        if let Some(i) = switch_to {
            self.active = i;
            self.view = BoardView::List;
            self.last_poll = Instant::now() - POLL_INTERVAL;
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.join_input).hint_text("board").desired_width(90.0));
            if ui.button("Join").clicked() && !self.join_input.trim().is_empty() {
                let board = self.join_input.trim().to_string();
                self.join_input.clear();
                self.join_board(app, board);
            }
        });
    }

    /// Deliberately not a plain `egui::ScrollArea::vertical().show(...)`
    /// loop -- that's what `ircapp.rs`'s own `scrollback()` uses, and
    /// it's fine for a chat channel's dozens of messages but not for a
    /// board's thousands of posts, since it lays out every row every
    /// frame regardless of what's actually scrolled into view.
    /// `show_rows` only lays out the rows in `row_range`.
    fn post_list(&mut self, ui: &mut egui::Ui, app: &AppContext) {
        let Some(board) = self.active_board() else {
            ui.weak("Join a board to see posts.");
            return;
        };
        let empty = Vec::new();
        let posts = self.posts.get(&board).unwrap_or(&empty);
        if posts.is_empty() {
            ui.weak("(no posts yet -- use New Post above)");
            return;
        }
        let row_height = 22.0;
        let mut open_target: Option<(String, String)> = None;
        egui::ScrollArea::vertical().id_salt(("board_post_list", self.instance_id())).show_rows(ui, row_height, posts.len(), |ui, row_range| {
            for i in row_range {
                let post = &posts[i];
                let link_marker = if post.has_link { " \u{1f517}" } else { "" };
                let label = format!("{}{}  \u{2014}  {}", post.title, link_marker, post.from);
                if ui.selectable_label(false, label).clicked() {
                    open_target = Some((board.clone(), post.id.clone()));
                }
            }
        });
        if let Some((board, post_id)) = open_target {
            self.open_post(app, board, post_id);
        }
    }

    /// Returns `true` if the user just asked to send the typed comment
    /// (button click or Enter) -- handled by the caller rather than
    /// inline here, since acting on it needs `&mut self` as a whole
    /// while this only ever has `self.view` borrowed.
    fn thread_view(&mut self, ui: &mut egui::Ui) -> bool {
        let instance_id = self.instance_id();
        let BoardView::Thread { post, comments, comment_input, .. } = &mut self.view else {
            return false;
        };
        let Some(post) = post else {
            ui.weak("Loading...");
            return false;
        };
        egui::ScrollArea::vertical().id_salt(("board_thread_scroll", instance_id)).show(ui, |ui| {
            ui.heading(&post.title);
            ui.label(format!("by {} \u{2014} {}", post.from, post.created_at.format("%Y-%m-%d %H:%M")));
            if let Some(url) = &post.link_url {
                ui.hyperlink(url);
            }
            if !post.body.is_empty() {
                ui.add_space(6.0);
                ui.label(&post.body);
            }
            ui.separator();
            ui.label(format!("{} comment(s)", comments.len()));
            for view in comments.iter() {
                let mark = if view.verified { "" } else { " [unverified]" };
                ui.label(format!("[{}] {}{}: {}", view.comment.created_at.format("%H:%M"), view.comment.from, mark, view.comment.body));
            }
        });
        let mut send = false;
        ui.horizontal(|ui| {
            let response = ui.add(egui::TextEdit::singleline(comment_input).desired_width(f32::INFINITY));
            let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Comment").clicked() || enter_pressed {
                send = true;
            }
        });
        send
    }

    fn content(&mut self, ui: &mut egui::Ui, app: &AppContext) {
        let in_thread = matches!(self.view, BoardView::Thread { .. });
        if in_thread {
            if self.thread_view(ui) {
                self.send_comment(app);
            }
        } else {
            self.post_list(ui, app);
        }
    }

    fn compose_dialog(&mut self, ctx: &egui::Context, app: &AppContext) {
        let Some(mut compose) = self.compose.take() else { return };
        let instance_id = self.instance_id();
        let can_post = !compose.title.trim().is_empty() && self.send_rx.is_none();
        let mut keep_open = true;
        let mut submit = false;
        egui::Window::new("New Post")
            .id(egui::Id::new(("board_compose", instance_id)))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Title:");
                ui.text_edit_singleline(&mut compose.title);
                ui.label("Link (optional):");
                ui.text_edit_singleline(&mut compose.link_url);
                ui.label("Body:");
                ui.add(egui::TextEdit::multiline(&mut compose.body).desired_rows(6).desired_width(f32::INFINITY));
                ui.horizontal(|ui| {
                    if ui.add_enabled(can_post, egui::Button::new("Post")).clicked() {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        keep_open = false;
                    }
                });
            });
        if submit {
            self.submit_post(app, compose);
            return;
        }
        if keep_open {
            self.compose = Some(compose);
        }
    }
}

impl RymApp for BoardApp {
    fn title(&self) -> String {
        "Board".to_string()
    }

    fn show(&mut self, ctx: &egui::Context, app: &AppContext, _launch: &mut Vec<Box<dyn RymApp>>) -> bool {
        if self.first_frame {
            self.first_frame = false;
            self.join_board(app, "general".to_string());
        }
        self.poll_list();
        self.poll_thread();
        self.poll_send(app);

        let in_thread = matches!(self.view, BoardView::Thread { .. });
        if !in_thread && self.list_rx.is_none() && self.active_board().is_some() && self.last_poll.elapsed() >= POLL_INTERVAL {
            self.refresh_active(app);
            self.last_poll = Instant::now();
        }
        // Same reasoning as `ircapp.rs`: without a scheduled repaint,
        // the elapsed() check above never gets re-evaluated with zero
        // user input.
        ctx.request_repaint_after(Duration::from_millis(500));

        let mut open = true;
        theme::DecoratedWindow::new(egui::Id::new(("board_window", self.instance_id())), self.title())
            .icon("\u{1f4cb}")
            .default_size([640.0, 460.0])
            .show(ctx, &mut open, |ui| {
                egui::Panel::left(egui::Id::new(("board_list_panel", self.instance_id())))
                    .default_size(140.0)
                    .resizable(false)
                    .show(ui, |ui| {
                        self.board_list(ui, app);
                    });
                egui::Panel::top(egui::Id::new(("board_header_panel", self.instance_id()))).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if in_thread && ui.button("\u{25c0} Back").clicked() {
                            self.view = BoardView::List;
                        }
                        if !in_thread && ui.button("New Post").clicked() && self.active_board().is_some() {
                            self.compose = Some(ComposePost::new());
                        }
                        if let Some(status) = &self.status {
                            let color = if self.status_is_error { theme::ERROR_MAROON } else { theme::TEXT_BLACK };
                            ui.colored_label(color, status);
                        }
                    });
                });
                self.content(ui, app);
            });

        self.compose_dialog(ctx, app);
        open
    }
}
