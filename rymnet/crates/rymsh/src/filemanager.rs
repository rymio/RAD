use crate::editor::{self, pad};
use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::{Attribute, Print, SetAttribute},
    terminal,
};
use std::cmp::Ordering;
use std::fs;
use std::io::{Write, stdout};
use std::path::PathBuf;
use walkdir::WalkDir;

const FOOTER: &str = " Tab switch pane  Enter open  Bksp up  c copy->other  m mkdir  n new file  r rename  d delete  e edit  q quit";

struct Entry {
    name: String,
    is_dir: bool,
    size: u64,
}

struct Pane {
    label: String,
    root: PathBuf,
    cwd: PathBuf,
    sandboxed: bool,
    entries: Vec<Entry>,
    selected: usize,
    top: usize,
}

impl Pane {
    fn new(label: impl Into<String>, root: PathBuf, sandboxed: bool) -> Self {
        let mut pane = Self {
            label: label.into(),
            root: root.clone(),
            cwd: root,
            sandboxed,
            entries: Vec::new(),
            selected: 0,
            top: 0,
        };
        pane.reload();
        pane
    }

    fn reload(&mut self) {
        let mut entries = Vec::new();
        if let Ok(read_dir) = fs::read_dir(&self.cwd) {
            for entry in read_dir.flatten() {
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                entries.push(Entry {
                    name: entry.file_name().to_string_lossy().to_string(),
                    is_dir: metadata.is_dir(),
                    size: metadata.len(),
                });
            }
        }
        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });
        self.entries = entries;
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
        self.top = 0;
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.selected_entry()
            .map(|entry| self.cwd.join(&entry.name))
    }

    fn move_selection(&mut self, delta: isize, list_height: usize) {
        if self.entries.is_empty() {
            return;
        }
        let len = self.entries.len() as isize;
        let next = (self.selected as isize + delta).clamp(0, len - 1) as usize;
        self.selected = next;
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + list_height {
            self.top = self.selected + 1 - list_height;
        }
    }

    fn enter(&mut self) {
        if let Some(entry) = self.selected_entry()
            && entry.is_dir
        {
            let name = entry.name.clone();
            self.cwd.push(name);
            self.reload();
        }
    }

    fn up_dir(&mut self) {
        if self.sandboxed && self.cwd == self.root {
            return;
        }
        if self.cwd.pop() {
            self.reload();
        }
    }
}

/// Runs a Midnight-Commander-style dual-pane file manager: `local_start` on
/// the left (the real machine, unsandboxed) and `account_root` on the right
/// (the logged-in user's encrypted rymnet virtual computer, sandboxed like
/// everywhere else in rymsh). Returns whether anything under `account_root`
/// changed, so the caller knows whether a `sync` is needed.
pub fn run(local_start: PathBuf, account_root: PathBuf, account_label: &str) -> Result<bool> {
    let mut left = Pane::new("computer", local_start, false);
    let mut right = Pane::new(format!("rymnet:{account_label}"), account_root, true);
    let mut active_left = true;
    let mut status = String::new();
    let mut right_dirty = false;

    terminal::enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, terminal::EnterAlternateScreen, cursor::Hide)?;

    let result = run_loop(
        &mut out,
        &mut left,
        &mut right,
        &mut active_left,
        &mut status,
        &mut right_dirty,
    );

    execute!(out, cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;
    result?;
    Ok(right_dirty)
}

#[allow(clippy::too_many_arguments)]
fn run_loop(
    out: &mut impl Write,
    left: &mut Pane,
    right: &mut Pane,
    active_left: &mut bool,
    status: &mut String,
    right_dirty: &mut bool,
) -> Result<()> {
    loop {
        let (_, rows) = terminal::size()?;
        let list_height = (rows.saturating_sub(3) as usize).max(1);
        draw(out, left, right, *active_left, status)?;
        status.clear();

        let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event::read()?
        else {
            continue;
        };

        match (code, modifiers) {
            (KeyCode::Tab, _) => *active_left = !*active_left,
            (KeyCode::Char('q'), KeyModifiers::NONE) | (KeyCode::F(10), _) => break,
            (KeyCode::Up, _) => {
                active_pane(left, right, *active_left).move_selection(-1, list_height)
            }
            (KeyCode::Down, _) => {
                active_pane(left, right, *active_left).move_selection(1, list_height)
            }
            (KeyCode::Enter, _) => active_pane(left, right, *active_left).enter(),
            (KeyCode::Backspace, _) => active_pane(left, right, *active_left).up_dir(),
            (KeyCode::F(5), _) | (KeyCode::Char('c'), KeyModifiers::NONE) => {
                let outcome = if *active_left {
                    copy_entry(left, right)
                } else {
                    copy_entry(right, left)
                };
                match outcome {
                    Ok(Some(name)) => {
                        *status = format!("Copied {name}");
                        if *active_left {
                            *right_dirty = true;
                        }
                    }
                    Ok(None) => {}
                    Err(err) => *status = format!("copy failed: {err:#}"),
                }
            }
            (KeyCode::F(7), _) | (KeyCode::Char('m'), KeyModifiers::NONE) => {
                if let Some(name) = prompt_line(out, "New folder name: ")? {
                    let pane = active_pane(left, right, *active_left);
                    match fs::create_dir(pane.cwd.join(&name)) {
                        Ok(()) => {
                            pane.reload();
                            *status = format!("Created {name}/");
                            if !*active_left {
                                *right_dirty = true;
                            }
                        }
                        Err(err) => *status = format!("mkdir failed: {err}"),
                    }
                }
            }
            (KeyCode::Char('n'), KeyModifiers::NONE) => {
                if let Some(name) = prompt_line(out, "New file name: ")? {
                    let pane = active_pane(left, right, *active_left);
                    match fs::write(pane.cwd.join(&name), b"") {
                        Ok(()) => {
                            pane.reload();
                            *status = format!("Created {name}");
                            if !*active_left {
                                *right_dirty = true;
                            }
                        }
                        Err(err) => *status = format!("create failed: {err}"),
                    }
                }
            }
            (KeyCode::F(8), _)
            | (KeyCode::Delete, _)
            | (KeyCode::Char('d'), KeyModifiers::NONE) => {
                let pane = active_pane(left, right, *active_left);
                if let Some(path) = pane.selected_path() {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if confirm(out, &format!("Delete {name}? (y/n)"))? {
                        let outcome = if path.is_dir() {
                            fs::remove_dir_all(&path)
                        } else {
                            fs::remove_file(&path)
                        };
                        let pane = active_pane(left, right, *active_left);
                        match outcome {
                            Ok(()) => {
                                pane.reload();
                                *status = format!("Deleted {name}");
                                if !*active_left {
                                    *right_dirty = true;
                                }
                            }
                            Err(err) => *status = format!("delete failed: {err}"),
                        }
                    }
                }
            }
            (KeyCode::F(2), _) | (KeyCode::Char('r'), KeyModifiers::NONE) => {
                let path = active_pane(left, right, *active_left).selected_path();
                if let Some(path) = path
                    && let Some(new_name) = prompt_line(out, "Rename to: ")?
                {
                    let pane = active_pane(left, right, *active_left);
                    match fs::rename(&path, pane.cwd.join(&new_name)) {
                        Ok(()) => {
                            pane.reload();
                            *status = format!("Renamed to {new_name}");
                            if !*active_left {
                                *right_dirty = true;
                            }
                        }
                        Err(err) => *status = format!("rename failed: {err}"),
                    }
                }
            }
            (KeyCode::F(4), _) | (KeyCode::Char('e'), KeyModifiers::NONE) => {
                let path = active_pane(left, right, *active_left).selected_path();
                if let Some(path) = path {
                    if path.is_dir() {
                        *status = "select a file to edit, not a directory".to_string();
                    } else {
                        let edit_result = editor::edit_inline(out, &path);
                        let pane = active_pane(left, right, *active_left);
                        pane.reload();
                        match edit_result {
                            Ok(()) => {
                                *status = "Saved.".to_string();
                                if !*active_left {
                                    *right_dirty = true;
                                }
                            }
                            Err(err) => *status = format!("edit failed: {err:#}"),
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn active_pane<'a>(left: &'a mut Pane, right: &'a mut Pane, active_left: bool) -> &'a mut Pane {
    if active_left { left } else { right }
}

/// Copies the selected entry (file or, recursively, a directory) from
/// `source`'s current directory into `dest`'s current directory. `dest`'s
/// cwd is always inside its sandbox root by construction (navigation is
/// clamped), so this can never write outside it.
fn copy_entry(source: &Pane, dest: &mut Pane) -> Result<Option<String>> {
    let Some(entry) = source.selected_entry() else {
        return Ok(None);
    };
    let name = entry.name.clone();
    let src_path = source.cwd.join(&name);
    let dst_path = dest.cwd.join(&name);
    if dst_path.exists() {
        anyhow::bail!("{name} already exists in the destination pane");
    }

    if src_path.is_dir() {
        for walked in WalkDir::new(&src_path) {
            let walked = walked?;
            let relative = walked.path().strip_prefix(&src_path)?;
            let target = dst_path.join(relative);
            if walked.file_type().is_dir() {
                fs::create_dir_all(&target)?;
            } else {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(walked.path(), &target)?;
            }
        }
    } else {
        fs::copy(&src_path, &dst_path)?;
    }

    dest.reload();
    Ok(Some(name))
}

fn draw(
    out: &mut impl Write,
    left: &Pane,
    right: &Pane,
    active_left: bool,
    status: &str,
) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    let half = cols / 2;
    let list_height = (rows.saturating_sub(3) as usize).max(1);

    queue!(out, terminal::Clear(terminal::ClearType::All))?;
    draw_pane(out, left, 0, half, list_height, active_left)?;
    draw_pane(out, right, half, cols - half, list_height, !active_left)?;

    let status_line = if status.is_empty() {
        " rymsh file manager".to_string()
    } else {
        format!(" {status}")
    };
    queue!(
        out,
        cursor::MoveTo(0, rows.saturating_sub(2)),
        Print(pad(&status_line, cols))
    )?;
    queue!(
        out,
        cursor::MoveTo(0, rows.saturating_sub(1)),
        Print(pad(FOOTER, cols))
    )?;
    out.flush()?;
    Ok(())
}

fn draw_pane(
    out: &mut impl Write,
    pane: &Pane,
    x: u16,
    width: u16,
    list_height: usize,
    active: bool,
) -> Result<()> {
    let title = format!(" {}: {}", pane.label, pane.cwd.display());
    queue!(out, cursor::MoveTo(x, 0))?;
    if active {
        queue!(out, SetAttribute(Attribute::Reverse))?;
    }
    queue!(out, Print(fit(&title, width)))?;
    if active {
        queue!(out, SetAttribute(Attribute::Reset))?;
    }

    for row in 0..list_height {
        queue!(out, cursor::MoveTo(x, (row + 1) as u16))?;
        match pane.entries.get(pane.top + row) {
            Some(entry) => {
                let marker = if entry.is_dir { "/" } else { "" };
                let label = if entry.is_dir {
                    format!(" {}{marker}", entry.name)
                } else {
                    format!(" {}{marker}  ({} bytes)", entry.name, entry.size)
                };
                let line = fit(&label, width);
                if active && pane.top + row == pane.selected {
                    queue!(
                        out,
                        SetAttribute(Attribute::Reverse),
                        Print(line),
                        SetAttribute(Attribute::Reset)
                    )?;
                } else {
                    queue!(out, Print(line))?;
                }
            }
            None => queue!(out, Print(fit("", width)))?,
        }
    }
    Ok(())
}

fn fit(text: &str, width: u16) -> String {
    let width = width as usize;
    if text.chars().count() >= width {
        text.chars().take(width).collect()
    } else {
        format!("{text}{}", " ".repeat(width - text.chars().count()))
    }
}

fn prompt_line(out: &mut impl Write, label: &str) -> Result<Option<String>> {
    let mut input = String::new();
    loop {
        let (cols, rows) = terminal::size()?;
        queue!(
            out,
            cursor::MoveTo(0, rows.saturating_sub(2)),
            Print(pad(&format!("{label}{input}"), cols))
        )?;
        out.flush()?;
        if let Event::Key(KeyEvent { code, .. }) = event::read()? {
            match code {
                KeyCode::Enter if !input.is_empty() => return Ok(Some(input)),
                KeyCode::Esc => return Ok(None),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => input.push(c),
                _ => {}
            }
        }
    }
}

fn confirm(out: &mut impl Write, label: &str) -> Result<bool> {
    let (cols, rows) = terminal::size()?;
    queue!(
        out,
        cursor::MoveTo(0, rows.saturating_sub(2)),
        Print(pad(label, cols))
    )?;
    out.flush()?;
    loop {
        if let Event::Key(KeyEvent { code, .. }) = event::read()? {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => return Ok(true),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => return Ok(false),
                _ => {}
            }
        }
    }
}
