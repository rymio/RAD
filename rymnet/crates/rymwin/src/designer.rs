//! Pure logic for Rymcode's Design view: no `egui`, no interpreter --
//! just string surgery on a `.win` file's marked layout block and
//! serialization of the `Widget` tree `rymscript.rs` already parses at
//! runtime. Kept separate from `rymcode.rs` deliberately, since this is
//! the one piece where a bug would silently corrupt a user's hand-
//! written code (see the module-level doc on `replace_layout_block`) --
//! it needs to be trivially unit-testable without any GUI context.
//!
//! The core idea: a `.win` file authored (even partly) via Design view
//! keeps a clearly delimited, auto-generated block -- a static `_LAYOUT`
//! Python literal plus a small dispatcher `draw()` -- and everything
//! outside that block is 100% the user's own code, never touched here.
//! Dynamic widget properties (a counter's changing label, a clock's
//! `now()`) go through *named functions the user writes*
//! (`text_<id>()`, `enabled_<id>()`), not an embedded expression
//! language -- the generated `draw()` looks each one up by name and
//! calls it if it exists, the same "real Python, not a template
//! string" spirit the rest of RymScript already has.

use crate::rymscript::Widget;

pub const LAYOUT_BEGIN: &str = "# --- rymcode:layout:begin (auto-generated -- edit via Design view) ---";
pub const LAYOUT_END: &str = "# --- rymcode:layout:end ---";

const DISPATCHER_TEMPLATE: &str = r#"
def _rymcode_walk(widgets):
    found = []
    for w in widgets:
        found.append(w)
        if "children" in w:
            found.extend(_rymcode_walk(w["children"]))
    return found


def draw(events):
    for w in _rymcode_walk(_LAYOUT):
        wid = w.get("id")
        if not wid:
            continue
        click = globals().get(f"on_{wid}_click")
        if click and events.get(wid):
            click(events)
        change = globals().get(f"on_{wid}_change")
        if change and wid in events:
            change(events[wid])
        text_fn = globals().get(f"text_{wid}")
        if text_fn:
            w["text"] = text_fn()
        enabled_fn = globals().get(f"enabled_{wid}")
        if enabled_fn:
            w["enabled"] = enabled_fn()
    return _LAYOUT
"#;

pub fn has_layout_block(buffer: &str) -> bool {
    buffer.contains(LAYOUT_BEGIN) && buffer.contains(LAYOUT_END)
}

/// Builds the full auto-generated block (markers included) for a given
/// widget tree.
pub fn generate_layout_block(widgets: &[Widget]) -> String {
    format!("{LAYOUT_BEGIN}\n{}\n{DISPATCHER_TEMPLATE}{LAYOUT_END}", serialize_layout(widgets))
}

/// Replaces the text strictly between the two markers with `new_block`
/// (which itself includes the markers), leaving every other byte in
/// `buffer` untouched -- if the markers already exist, only the span
/// between and including them is replaced; nothing before or after it
/// is read, reformatted, or reflowed. If no markers exist yet (a brand
/// new file, or a hand-written script that's never used Design view),
/// the block is inserted at the very top instead.
pub fn replace_layout_block(buffer: &str, new_block: &str) -> String {
    if let Some(start) = buffer.find(LAYOUT_BEGIN)
        && let Some(end_rel) = buffer[start..].find(LAYOUT_END)
    {
        let end = start + end_rel + LAYOUT_END.len();
        return format!("{}{}{}", &buffer[..start], new_block, &buffer[end..]);
    }
    if buffer.trim().is_empty() {
        new_block.to_string()
    } else {
        format!("{new_block}\n\n{buffer}")
    }
}

fn python_str_literal(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn serialize_widget(widget: &Widget, indent: usize) -> String {
    match widget {
        Widget::Heading(text) => format!("{{\"type\": \"heading\", \"text\": {}}}", python_str_literal(text)),
        Widget::Label(text) => format!("{{\"type\": \"label\", \"text\": {}}}", python_str_literal(text)),
        Widget::Separator => "{\"type\": \"separator\"}".to_string(),
        Widget::Button { id, text, color, enabled } => {
            let mut out = format!("{{\"type\": \"button\", \"id\": {}, \"text\": {}", python_str_literal(id), python_str_literal(text));
            if let Some([r, g, b]) = color {
                out.push_str(&format!(", \"color\": [{r}, {g}, {b}]"));
            }
            if !enabled {
                out.push_str(", \"enabled\": False");
            }
            out.push('}');
            out
        }
        Widget::TextInput { id, value } => {
            format!("{{\"type\": \"text_input\", \"id\": {}, \"value\": {}}}", python_str_literal(id), python_str_literal(value))
        }
        Widget::Checkbox { id, text, value } => format!(
            "{{\"type\": \"checkbox\", \"id\": {}, \"text\": {}, \"value\": {}}}",
            python_str_literal(id),
            python_str_literal(text),
            if *value { "True" } else { "False" }
        ),
        Widget::Row(children) => serialize_container("row", None, children, indent),
        Widget::Column(children) => serialize_container("column", None, children, indent),
        Widget::Grid { id, columns, children } => serialize_container("grid", Some((id.as_str(), *columns)), children, indent),
    }
}

fn serialize_container(kind: &str, grid_meta: Option<(&str, usize)>, children: &[Widget], indent: usize) -> String {
    let child_pad = "    ".repeat(indent + 1);
    let closing_pad = "    ".repeat(indent);
    let mut out = format!("{{\"type\": \"{kind}\"");
    if let Some((id, columns)) = grid_meta {
        out.push_str(&format!(", \"id\": {}, \"columns\": {columns}", python_str_literal(id)));
    }
    if children.is_empty() {
        out.push_str(", \"children\": []}");
        return out;
    }
    out.push_str(", \"children\": [\n");
    for child in children {
        out.push_str(&child_pad);
        out.push_str(&serialize_widget(child, indent + 1));
        out.push_str(",\n");
    }
    out.push_str(&closing_pad);
    out.push_str("]}");
    out
}

/// The `_LAYOUT = [...]` literal itself, without the surrounding marker
/// comments or dispatcher -- exposed separately so it's testable on its
/// own, in addition to the round trip through `generate_layout_block`.
pub fn serialize_layout(widgets: &[Widget]) -> String {
    if widgets.is_empty() {
        return "_LAYOUT = []".to_string();
    }
    let mut out = String::from("_LAYOUT = [\n");
    for widget in widgets {
        out.push_str("    ");
        out.push_str(&serialize_widget(widget, 1));
        out.push_str(",\n");
    }
    out.push(']');
    out
}

/// A path into a (possibly nested) widget tree: `[2]` is the third
/// top-level widget, `[1, 0]` is the first child of the second
/// top-level widget, and so on. Used by Design view to track which
/// widget is currently selected without holding a live borrow into the
/// tree across frames.
pub type WidgetPath = Vec<usize>;

pub fn widget_at<'a>(widgets: &'a [Widget], path: &[usize]) -> Option<&'a Widget> {
    let (&first, rest) = path.split_first()?;
    let widget = widgets.get(first)?;
    if rest.is_empty() {
        return Some(widget);
    }
    children_of(widget).and_then(|children| widget_at(children, rest))
}

pub fn widget_at_mut<'a>(widgets: &'a mut [Widget], path: &[usize]) -> Option<&'a mut Widget> {
    let (&first, rest) = path.split_first()?;
    let widget = widgets.get_mut(first)?;
    if rest.is_empty() {
        return Some(widget);
    }
    children_of_mut(widget).and_then(|children| widget_at_mut(children, rest))
}

/// The mutable children `Vec` a new widget should be appended to for a
/// given container path -- `&[]` means "append to the root list".
pub fn children_at_mut<'a>(widgets: &'a mut Vec<Widget>, path: &[usize]) -> Option<&'a mut Vec<Widget>> {
    if path.is_empty() {
        return Some(widgets);
    }
    let (&first, rest) = path.split_first()?;
    let widget = widgets.get_mut(first)?;
    if rest.is_empty() {
        return children_of_mut(widget);
    }
    children_of_mut(widget).and_then(|children| children_at_mut(children, rest))
}

/// Removes the widget at `path`, if it exists.
pub fn remove_widget(widgets: &mut Vec<Widget>, path: &[usize]) {
    let Some((&last, parent_path)) = path.split_last() else { return };
    if let Some(parent_children) = children_at_mut(widgets, parent_path)
        && last < parent_children.len()
    {
        parent_children.remove(last);
    }
}

pub fn children_of(widget: &Widget) -> Option<&[Widget]> {
    match widget {
        Widget::Row(children) | Widget::Column(children) => Some(children),
        Widget::Grid { children, .. } => Some(children),
        _ => None,
    }
}

pub fn is_container(widget: &Widget) -> bool {
    children_of(widget).is_some()
}

/// A short, human-readable summary for one row of Design view's
/// component tree -- e.g. `Button: "Click me" (id: btn1)`.
pub fn design_summary(widget: &Widget) -> String {
    match widget {
        Widget::Heading(text) => format!("Heading: \"{text}\""),
        Widget::Label(text) => format!("Label: \"{text}\""),
        Widget::Separator => "Separator".to_string(),
        Widget::Button { id, text, .. } => format!("Button: \"{text}\" (id: {id})"),
        Widget::TextInput { id, .. } => format!("Text input (id: {id})"),
        Widget::Checkbox { id, text, .. } => format!("Checkbox: \"{text}\" (id: {id})"),
        Widget::Row(children) => format!("Row ({} widget{})", children.len(), if children.len() == 1 { "" } else { "s" }),
        Widget::Column(children) => format!("Column ({} widget{})", children.len(), if children.len() == 1 { "" } else { "s" }),
        Widget::Grid { id, columns, children } => format!("Grid (id: {id}, {columns} columns, {} widget{})", children.len(), if children.len() == 1 { "" } else { "s" }),
    }
}

fn children_of_mut(widget: &mut Widget) -> Option<&mut Vec<Widget>> {
    match widget {
        Widget::Row(children) | Widget::Column(children) => Some(children),
        Widget::Grid { children, .. } => Some(children),
        _ => None,
    }
}

/// A fresh, sensibly-defaulted widget of each kind the Component
/// Palette can add -- `id_seed` is folded into generated ids
/// (`button_3`, `grid_3`, ...) so repeated adds don't collide.
pub fn default_widget(kind: &str, id_seed: usize) -> Widget {
    match kind {
        "heading" => Widget::Heading("Heading".to_string()),
        "label" => Widget::Label("Label".to_string()),
        "separator" => Widget::Separator,
        "button" => Widget::Button { id: format!("button_{id_seed}"), text: "Button".to_string(), color: None, enabled: true },
        "text_input" => Widget::TextInput { id: format!("input_{id_seed}"), value: String::new() },
        "checkbox" => Widget::Checkbox { id: format!("checkbox_{id_seed}"), text: "Checkbox".to_string(), value: false },
        "row" => Widget::Row(Vec::new()),
        "column" => Widget::Column(Vec::new()),
        "grid" => Widget::Grid { id: format!("grid_{id_seed}"), columns: 2, children: Vec::new() },
        _ => Widget::Label("Label".to_string()),
    }
}

/// The `id` a widget exposes for event dispatch/handler naming, if it
/// has one (containers and the id-less leaf types don't).
pub fn widget_id(widget: &Widget) -> Option<&str> {
    match widget {
        Widget::Button { id, .. } | Widget::TextInput { id, .. } | Widget::Checkbox { id, .. } | Widget::Grid { id, .. } => Some(id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_widgets() -> Vec<Widget> {
        vec![
            Widget::Heading("Title".to_string()),
            Widget::Row(vec![
                Widget::Button { id: "a".to_string(), text: "A".to_string(), color: None, enabled: true },
                Widget::Button { id: "b".to_string(), text: "B \"quoted\"".to_string(), color: Some([10, 20, 30]), enabled: false },
            ]),
        ]
    }

    #[test]
    fn replace_layout_block_touches_only_the_marked_span() {
        let buffer = format!(
            "# a user comment above\nuser_var = 1\n\n{}\nold body\n{}\n\ndef on_a_click(events):\n    pass\n",
            LAYOUT_BEGIN, LAYOUT_END
        );
        let before_marker = buffer[..buffer.find(LAYOUT_BEGIN).unwrap()].to_string();
        let after_marker = buffer[buffer.find(LAYOUT_END).unwrap() + LAYOUT_END.len()..].to_string();

        let new_block = generate_layout_block(&sample_widgets());
        let updated = replace_layout_block(&buffer, &new_block);

        assert!(updated.starts_with(&before_marker), "text before the markers must be byte-identical");
        assert!(updated.ends_with(&after_marker), "text after the markers must be byte-identical");
        assert!(updated.contains("_LAYOUT = ["), "the new block should be spliced in between");
        assert!(!updated.contains("old body"), "the old block content should be gone");
    }

    #[test]
    fn replace_layout_block_inserts_at_top_when_no_markers_exist() {
        let buffer = "def on_a_click(events):\n    pass\n";
        let new_block = generate_layout_block(&sample_widgets());
        let updated = replace_layout_block(buffer, &new_block);
        assert!(updated.starts_with(LAYOUT_BEGIN));
        assert!(updated.ends_with(buffer), "existing user code should be preserved, unchanged, after the new block");
    }

    #[test]
    fn serialize_layout_escapes_quotes_and_round_trips_through_the_real_parser() {
        let widgets = sample_widgets();
        let source = format!("{}\n\ndef draw(events):\n    return _LAYOUT\n", serialize_layout(&widgets));

        // Not just string-shape assertions -- actually load it through
        // the real interpreter and confirm the parsed structure matches
        // what went in, the same "prove it, don't assume it" standard
        // this file's Phase 1 tests already hold to.
        let runner = crate::rymscript::WinAppRunner::load(&source, std::env::temp_dir()).expect("generated layout literal must be valid, loadable RymScript");
        let parsed = runner.frame(&std::collections::HashMap::new()).expect("frame");

        assert!(matches!(&parsed[0], Widget::Heading(text) if text == "Title"));
        let Widget::Row(children) = &parsed[1] else { panic!("expected a Row") };
        assert!(matches!(&children[0], Widget::Button { id, color, enabled, .. } if id == "a" && color.is_none() && *enabled));
        assert!(matches!(
            &children[1],
            Widget::Button { id, text, color: Some([10, 20, 30]), enabled: false } if id == "b" && text == "B \"quoted\""
        ));
    }

    #[test]
    fn generated_dispatcher_calls_named_handler_and_text_functions() {
        let widgets = vec![Widget::Button { id: "inc".to_string(), text: "placeholder".to_string(), color: None, enabled: true }];
        let block = generate_layout_block(&widgets);
        let source = format!(
            "count = 0\n\n{block}\n\ndef on_inc_click(events):\n    global count\n    count += 1\n\ndef text_inc():\n    return f\"Count: {{count}}\"\n"
        );

        let runner = crate::rymscript::WinAppRunner::load(&source, std::env::temp_dir()).expect("load generated program");
        let widgets = runner.frame(&std::collections::HashMap::new()).expect("first frame");
        assert!(matches!(&widgets[0], Widget::Button { text, .. } if text == "Count: 0"));

        let mut events = std::collections::HashMap::new();
        events.insert("inc".to_string(), crate::rymscript::EventValue::Clicked);
        let widgets = runner.frame(&events).expect("frame after click");
        assert!(
            matches!(&widgets[0], Widget::Button { text, .. } if text == "Count: 1"),
            "the click handler should run and the text function should reflect the updated state"
        );
    }

    #[test]
    fn widget_path_lookup_finds_and_mutates_nested_widgets() {
        let mut widgets = sample_widgets();
        assert!(matches!(widget_at(&widgets, &[0]), Some(Widget::Heading(text)) if text == "Title"));
        assert!(matches!(widget_at(&widgets, &[1, 1]), Some(Widget::Button { id, .. }) if id == "b"));
        assert!(widget_at(&widgets, &[1, 5]).is_none());

        if let Some(Widget::Button { text, .. }) = widget_at_mut(&mut widgets, &[1, 0]) {
            *text = "Changed".to_string();
        } else {
            panic!("expected to find and mutate widget at [1, 0]");
        }
        assert!(matches!(widget_at(&widgets, &[1, 0]), Some(Widget::Button { text, .. }) if text == "Changed"));
    }

    #[test]
    fn remove_widget_deletes_only_the_targeted_node() {
        let mut widgets = sample_widgets();
        remove_widget(&mut widgets, &[1, 0]);
        let Widget::Row(children) = &widgets[1] else { panic!("expected a Row") };
        assert_eq!(children.len(), 1);
        assert!(matches!(&children[0], Widget::Button { id, .. } if id == "b"), "the remaining child should be the one that wasn't removed");
    }

    #[test]
    fn children_at_mut_targets_the_root_for_an_empty_path() {
        let mut widgets = sample_widgets();
        let root = children_at_mut(&mut widgets, &[]).expect("root should always resolve");
        root.push(Widget::Separator);
        assert_eq!(widgets.len(), 3);
    }
}
