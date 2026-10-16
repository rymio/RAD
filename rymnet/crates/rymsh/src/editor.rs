use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::Print,
    terminal,
};
use std::fs;
use std::io::{Write, stdout};
use std::path::Path;

/// A small full-screen text editor in the spirit of nano: arrow-key
/// navigation, Ctrl+S to save, Ctrl+X to exit (confirming on unsaved
/// changes). It does not attempt nano's full feature set (search, cut/paste
/// buffers, undo) -- just enough to comfortably edit a file from rymsh.
pub fn edit(path: &Path) -> Result<()> {
    terminal::enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, terminal::EnterAlternateScreen, cursor::Hide)?;

    let result = edit_inline(&mut out, path);

    execute!(out, cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;
    result
}

/// Same editor, but assumes the caller already owns raw mode and the
/// alternate screen (e.g. the dual-pane file manager dropping into an
/// editor without leaving its own full-screen view).
pub fn edit_inline(out: &mut impl Write, path: &Path) -> Result<()> {
    let original = fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = if original.is_empty() {
        vec![String::new()]
    } else {
        original.lines().map(str::to_string).collect()
    };
    let mut modified = false;
    let mut row = 0usize;
    let mut col = 0usize;
    let mut top = 0usize;
    let mut status_message = String::new();

    run_loop(
        out,
        &mut lines,
        &mut row,
        &mut col,
        &mut top,
        &mut modified,
        &mut status_message,
        path,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_loop(
    out: &mut impl Write,
    lines: &mut Vec<String>,
    row: &mut usize,
    col: &mut usize,
    top: &mut usize,
    modified: &mut bool,
    status_message: &mut String,
    path: &Path,
) -> Result<()> {
    loop {
        draw(out, lines, *row, *col, top, path, *modified, status_message)?;
        status_message.clear();

        let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event::read()?
        else {
            continue;
        };

        match (code, modifiers) {
            (KeyCode::Char('x'), KeyModifiers::CONTROL) => {
                if !*modified || confirm_discard(out)? {
                    break;
                }
                status_message.push_str("Discarded. Press Ctrl+X again to exit, or keep editing.");
            }
            (KeyCode::Char('s'), KeyModifiers::CONTROL)
            | (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                let mut text = lines.join("\n");
                text.push('\n');
                fs::write(path, text)?;
                *modified = false;
                status_message.push_str("Saved.");
            }
            (KeyCode::Char(c), KeyModifiers::NONE) | (KeyCode::Char(c), KeyModifiers::SHIFT) => {
                lines[*row].insert(*col, c);
                *col += 1;
                *modified = true;
            }
            (KeyCode::Enter, _) => {
                let rest = lines[*row].split_off(*col);
                lines.insert(*row + 1, rest);
                *row += 1;
                *col = 0;
                *modified = true;
            }
            (KeyCode::Tab, _) => {
                lines[*row].insert_str(*col, "    ");
                *col += 4;
                *modified = true;
            }
            (KeyCode::Backspace, _) => {
                if *col > 0 {
                    lines[*row].remove(*col - 1);
                    *col -= 1;
                    *modified = true;
                } else if *row > 0 {
                    let current = lines.remove(*row);
                    *row -= 1;
                    *col = lines[*row].len();
                    lines[*row].push_str(&current);
                    *modified = true;
                }
            }
            (KeyCode::Delete, _) => {
                if *col < lines[*row].len() {
                    lines[*row].remove(*col);
                    *modified = true;
                } else if *row + 1 < lines.len() {
                    let next = lines.remove(*row + 1);
                    lines[*row].push_str(&next);
                    *modified = true;
                }
            }
            (KeyCode::Left, _) => {
                if *col > 0 {
                    *col -= 1;
                } else if *row > 0 {
                    *row -= 1;
                    *col = lines[*row].len();
                }
            }
            (KeyCode::Right, _) => {
                if *col < lines[*row].len() {
                    *col += 1;
                } else if *row + 1 < lines.len() {
                    *row += 1;
                    *col = 0;
                }
            }
            (KeyCode::Up, _) if *row > 0 => {
                *row -= 1;
                *col = (*col).min(lines[*row].len());
            }
            (KeyCode::Down, _) if *row + 1 < lines.len() => {
                *row += 1;
                *col = (*col).min(lines[*row].len());
            }
            (KeyCode::Home, _) => *col = 0,
            (KeyCode::End, _) => *col = lines[*row].len(),
            _ => {}
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw(
    out: &mut impl Write,
    lines: &[String],
    row: usize,
    col: usize,
    top: &mut usize,
    path: &Path,
    modified: bool,
    status_message: &str,
) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    let view_height = rows.saturating_sub(2) as usize;
    if view_height == 0 {
        return Ok(());
    }
    if row < *top {
        *top = row;
    } else if row >= *top + view_height {
        *top = row + 1 - view_height;
    }

    queue!(out, terminal::Clear(terminal::ClearType::All))?;
    for i in 0..view_height {
        queue!(out, cursor::MoveTo(0, i as u16))?;
        match lines.get(*top + i) {
            Some(line) => {
                let display: String = line.chars().take(cols as usize).collect();
                queue!(out, Print(display))?;
            }
            None => queue!(out, Print("~"))?,
        }
    }

    let title = format!(
        " rymsh nano-clone   {}{}",
        path.display(),
        if modified { "  [Modified]" } else { "" }
    );
    queue!(
        out,
        cursor::MoveTo(0, rows.saturating_sub(2)),
        Print(pad(&title, cols))
    )?;
    let footer = if status_message.is_empty() {
        " ^S Save   ^X Exit   arrows: move".to_string()
    } else {
        format!(" {status_message}")
    };
    queue!(
        out,
        cursor::MoveTo(0, rows.saturating_sub(1)),
        Print(pad(&footer, cols))
    )?;
    queue!(
        out,
        cursor::MoveTo(
            (col as u16).min(cols.saturating_sub(1)),
            (row - *top) as u16
        )
    )?;
    out.flush()?;
    Ok(())
}

pub(crate) fn pad(text: &str, width: u16) -> String {
    let width = width as usize;
    if text.len() >= width {
        text.chars().take(width).collect()
    } else {
        format!("{text}{}", " ".repeat(width - text.len()))
    }
}

fn confirm_discard(out: &mut impl Write) -> Result<bool> {
    let (cols, rows) = terminal::size()?;
    queue!(
        out,
        cursor::MoveTo(0, rows.saturating_sub(1)),
        Print(pad(" Unsaved changes. Exit without saving? (y/n)", cols))
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
