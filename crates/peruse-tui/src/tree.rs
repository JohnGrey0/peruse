//! The tree that the record view draws for one row.
//!
//! A JSON file, and a Parquet file with a structure inside it, hold a value
//! that holds other values. The grid can only show such a value as one long
//! text:
//!
//! ```text
//! {'id': 665991, 'login': petroav, 'url': 'https://api.github.com/users/pe…
//! ```
//!
//! That text says what the value holds, but the user cannot read one field of
//! it, cannot copy one field, and cannot filter on one field. This module
//! makes a tree from the row instead. Each field is one line, and a line that
//! holds other values opens and closes.
//!
//! The engine gives the row as JSON, because JSON has one rule for a value
//! that holds a quotation mark. The text of a structure that DuckDB writes has
//! no such rule, and Peruse would have to write a parser for it.
//!
//! This module holds no terminal code. It gives a list of lines, and
//! `overlays.rs` draws them.

use peruse_core::query::{path_text, Step};
use serde_json::Value;
use std::collections::HashSet;

/// The deepest level that the tree opens.
///
/// A file cannot make a tree deeper than this, but a limit stops a value that
/// points to itself from filling the memory.
const MAX_DEPTH: usize = 64;

/// What one line of the tree holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// A structure with this number of fields.
    Object(usize),
    /// A list with this number of items.
    Array(usize),
    /// One value.
    Leaf,
}

impl NodeKind {
    /// Gives `true` when the line can open.
    pub fn opens(self) -> bool {
        match self {
            NodeKind::Object(n) | NodeKind::Array(n) => n > 0,
            NodeKind::Leaf => false,
        }
    }
}

/// The family of a value, for the color that the record view paints it with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// The value is missing.
    Null,
    /// A text with no character.
    Empty,
    /// A number.
    Number,
    /// A value that is true or false.
    Bool,
    /// Text.
    Text,
    /// A structure or a list.
    Nested,
}

/// One line that the record view draws.
#[derive(Clone, Debug)]
pub struct Line {
    /// The level of the line. The columns of the row have the level 0.
    pub depth: usize,
    /// The name of the field, or the position in a list, as `[3]`.
    pub label: String,
    /// What the line holds.
    pub kind: NodeKind,
    /// The value as text, or a short form for a structure or a list.
    pub value: String,
    /// The family of the value.
    pub family: Family,
    /// The path from the row to this value.
    pub path: Vec<Step>,
    /// `true` when the line is open.
    pub open: bool,
}

impl Line {
    /// Gives the path as a person reads it, such as `payload.commits[0].sha`.
    pub fn path_text(&self) -> String {
        path_text(&self.path)
    }
}

/// One row, as a tree that opens and closes.
#[derive(Clone, Debug)]
pub struct Tree {
    root: Option<Value>,
    /// The path of each line that is open, as text.
    open: HashSet<String>,
    /// `true` while the tree hides the fields that hold no value.
    pub hide_empty: bool,
    /// The message from a row that the module could not read.
    pub error: Option<String>,
}

/// The tree with no row in it.
///
/// The rule about the empty fields must be the same here as in [`Tree::new`].
/// The record view makes an empty tree while it waits for the engine, and a
/// different rule there would change the view when the row arrives.
impl Default for Tree {
    fn default() -> Tree {
        Tree {
            root: None,
            open: HashSet::new(),
            hide_empty: true,
            error: None,
        }
    }
}

impl Tree {
    /// Makes a tree from one row of JSON.
    ///
    /// The tree starts with the columns of the row open, because those are
    /// the values that the user came to see. A structure inside a column
    /// starts closed.
    ///
    /// The tree hides the fields that hold no value at the start. A structure
    /// that DuckDB reads from a JSON file holds one field for each field of
    /// each row of the file. One row therefore holds a value in a few of them,
    /// and NULL in the others. Those NULLs are the absence of a field in that
    /// row, and not a missing value, so the first screen is more useful
    /// without them. Each line that holds a structure says how many fields it
    /// hides, and the key `z` shows them.
    pub fn new(json: &str) -> Tree {
        match serde_json::from_str::<Value>(json) {
            Ok(root) => Tree {
                root: Some(root),
                open: HashSet::new(),
                hide_empty: true,
                error: None,
            },
            Err(e) => Tree {
                root: None,
                open: HashSet::new(),
                hide_empty: true,
                error: Some(e.to_string()),
            },
        }
    }

    /// Gives `true` when the tree holds no row yet.
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Puts another row in the tree, and keeps the lines that are open.
    ///
    /// The user moves from row to row to compare one field. A tree that
    /// closed at each row would make that impossible.
    pub fn with_row(self, json: &str) -> Tree {
        let mut t = Tree::new(json);
        t.open = self.open;
        t.hide_empty = self.hide_empty;
        t
    }

    /// Opens a line, or closes it.
    pub fn toggle(&mut self, path: &[Step]) {
        let key = path_text(path);
        if !self.open.remove(&key) {
            self.open.insert(key);
        }
    }

    /// Opens a line.
    pub fn open_path(&mut self, path: &[Step]) {
        self.open.insert(path_text(path));
    }

    /// Closes a line.
    pub fn close_path(&mut self, path: &[Step]) {
        self.open.remove(&path_text(path));
    }

    /// Gives `true` when a line is open.
    pub fn is_open(&self, path: &[Step]) -> bool {
        self.open.contains(&path_text(path))
    }

    /// Closes each line.
    pub fn collapse_all(&mut self) {
        self.open.clear();
    }

    /// Opens each line that holds other values.
    pub fn expand_all(&mut self) {
        let Some(root) = self.root.clone() else { return };
        let mut paths = Vec::new();
        collect_paths(&root, &mut Vec::new(), 0, &mut paths);
        for p in paths {
            self.open.insert(p);
        }
    }

    /// Gives the lines that the record view draws.
    ///
    /// The text `find` keeps the lines whose name or value holds it. A line
    /// that holds a match below it stays, and it opens, so the user sees the
    /// way to the match.
    pub fn lines(&self, find: &str) -> Vec<Line> {
        let Some(root) = &self.root else {
            return Vec::new();
        };
        let needle = find.trim().to_lowercase();
        let mut out = Vec::new();
        // The row itself is the first structure. Its fields are the columns,
        // and the view draws them at the level 0.
        match root {
            Value::Object(map) => {
                for (k, v) in map {
                    walk(
                        self,
                        &needle,
                        Step::Field(k.clone()),
                        v,
                        &mut Vec::new(),
                        0,
                        &mut out,
                    );
                }
            }
            other => walk(
                self,
                &needle,
                Step::Field("value".into()),
                other,
                &mut Vec::new(),
                0,
                &mut out,
            ),
        }
        out
    }

    /// Gives the value at a path, as text that the user can copy.
    ///
    /// A structure or a list comes back as JSON with line ends and spaces, so
    /// the user can read it.
    pub fn value_at(&self, path: &[Step]) -> Option<String> {
        let mut v = self.root.as_ref()?;
        for s in path {
            v = match (s, v) {
                (Step::Field(name), Value::Object(m)) => m.get(name)?,
                (Step::Index(i), Value::Array(a)) => a.get(*i)?,
                _ => return None,
            };
        }
        Some(match v {
            Value::Null => "NULL".to_string(),
            Value::String(s) => s.clone(),
            Value::Object(_) | Value::Array(_) => {
                serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
            }
            other => other.to_string(),
        })
    }

    /// Gives the complete row as JSON, with line ends and spaces.
    pub fn as_json(&self) -> Option<String> {
        let root = self.root.as_ref()?;
        serde_json::to_string_pretty(root).ok()
    }
}

/// Collects the path of each value that holds other values.
fn collect_paths(v: &Value, path: &mut Vec<Step>, depth: usize, out: &mut Vec<String>) {
    if depth >= MAX_DEPTH {
        return;
    }
    match v {
        Value::Object(m) => {
            if !path.is_empty() {
                out.push(path_text(path));
            }
            for (k, child) in m {
                path.push(Step::Field(k.clone()));
                collect_paths(child, path, depth + 1, out);
                path.pop();
            }
        }
        Value::Array(a) => {
            if !path.is_empty() {
                out.push(path_text(path));
            }
            for (i, child) in a.iter().enumerate() {
                path.push(Step::Index(i));
                collect_paths(child, path, depth + 1, out);
                path.pop();
            }
        }
        _ => {}
    }
}

/// Gives `true` when a value, or a value below it, holds the text.
fn matches(step: &Step, v: &Value, needle: &str, depth: usize) -> bool {
    if needle.is_empty() {
        return true;
    }
    let label = match step {
        Step::Field(n) => n.to_lowercase(),
        Step::Index(i) => format!("[{i}]"),
    };
    if label.contains(needle) {
        return true;
    }
    if depth >= MAX_DEPTH {
        return false;
    }
    match v {
        Value::Object(m) => m
            .iter()
            .any(|(k, c)| matches(&Step::Field(k.clone()), c, needle, depth + 1)),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .any(|(i, c)| matches(&Step::Index(i), c, needle, depth + 1)),
        Value::Null => false,
        Value::String(s) => s.to_lowercase().contains(needle),
        other => other.to_string().to_lowercase().contains(needle),
    }
}

/// Adds one value to the list of lines, and then the values below it.
fn walk(
    tree: &Tree,
    needle: &str,
    step: Step,
    v: &Value,
    path: &mut Vec<Step>,
    depth: usize,
    out: &mut Vec<Line>,
) {
    if depth >= MAX_DEPTH {
        return;
    }
    // A field with no value inside a structure is the absence of that field
    // in this row, and not a missing value. The tree hides it until the user
    // asks for it.
    //
    // A column of the row is a different case. The schema declares it, so a
    // NULL there is a value that the row does not have. The record view shows
    // each column always, as the grid does.
    if tree.hide_empty && depth > 0 && v.is_null() {
        return;
    }
    if !matches(&step, v, needle, depth) {
        return;
    }

    path.push(step.clone());
    let label = match &step {
        Step::Field(n) => n.clone(),
        Step::Index(i) => format!("[{i}]"),
    };

    let (kind, value, family) = match v {
        Value::Null => (NodeKind::Leaf, "NULL".to_string(), Family::Null),
        Value::Bool(b) => (NodeKind::Leaf, b.to_string(), Family::Bool),
        Value::Number(n) => (NodeKind::Leaf, n.to_string(), Family::Number),
        Value::String(s) if s.is_empty() => {
            (NodeKind::Leaf, "(empty)".to_string(), Family::Empty)
        }
        Value::String(s) => (NodeKind::Leaf, s.clone(), Family::Text),
        Value::Object(m) => {
            let set = m.values().filter(|c| !c.is_null()).count();
            let hidden = m.len() - set;
            // Say how many fields the line hides. The user then knows that
            // the key `z` has something to show.
            let text = if tree.hide_empty && hidden > 0 {
                format!("{{{set} of {} fields}}", m.len())
            } else {
                format!("{{{} fields}}", m.len())
            };
            (NodeKind::Object(m.len()), text, Family::Nested)
        }
        Value::Array(a) => (
            NodeKind::Array(a.len()),
            format!("[{} items]", a.len()),
            Family::Nested,
        ),
    };

    // A search opens the way to each match, so the user sees it at once.
    let open = kind.opens() && (tree.is_open(path) || !needle.is_empty());
    out.push(Line {
        depth,
        label,
        kind,
        value,
        family,
        path: path.clone(),
        open,
    });

    if open {
        match v {
            Value::Object(m) => {
                for (k, child) in m {
                    walk(
                        tree,
                        needle,
                        Step::Field(k.clone()),
                        child,
                        path,
                        depth + 1,
                        out,
                    );
                }
            }
            Value::Array(a) => {
                for (i, child) in a.iter().enumerate() {
                    walk(tree, needle, Step::Index(i), child, path, depth + 1, out);
                }
            }
            _ => {}
        }
    }
    path.pop();
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: &str = r#"{
        "id": "249",
        "actor": {"id": 665991, "login": "petroav", "gravatar_id": ""},
        "payload": {"ref": "master", "push_id": null, "size": null,
                    "commits": [{"sha": "aa", "message": "one"},
                                {"sha": "bb", "message": "two"}]},
        "public": true,
        "org": null
    }"#;

    fn labels(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| format!("{}{}", "  ".repeat(l.depth), l.label))
            .collect()
    }

    #[test]
    fn the_columns_of_the_row_are_the_first_level() {
        let t = Tree::new(ROW);
        let lines = t.lines("");
        // The column `org` holds no value, and the view shows it. The schema
        // declares that column, so the NULL is a value that the row does not
        // have, as it is in the grid.
        assert_eq!(labels(&lines), ["id", "actor", "payload", "public", "org"]);
        assert!(lines.iter().all(|l| l.depth == 0));
    }

    #[test]
    fn a_field_with_no_value_inside_a_structure_is_hidden_at_the_start() {
        // The fields `push_id` and `size` of the payload hold no value. Such
        // a field is the absence of a field in this row, and not a missing
        // value, so the first screen is more useful without them.
        let mut t = Tree::new(ROW);
        t.open_path(&[Step::Field("payload".into())]);
        let found = labels(&t.lines(""));
        assert!(found.contains(&"  ref".to_string()), "{found:?}");
        assert!(!found.contains(&"  push_id".to_string()), "{found:?}");

        t.hide_empty = false;
        let found = labels(&t.lines(""));
        assert!(found.contains(&"  push_id".to_string()), "{found:?}");
    }

    #[test]
    fn the_fields_keep_the_order_of_the_file() {
        // Without the feature `preserve_order`, the fields would come back in
        // the order of the alphabet, and the record view would not agree with
        // the grid.
        let t = Tree::new(r#"{"zebra": 1, "apple": 2, "mango": 3}"#);
        assert_eq!(labels(&t.lines("")), ["zebra", "apple", "mango"]);
    }

    #[test]
    fn a_structure_opens_and_closes() {
        let mut t = Tree::new(ROW);
        let actor = vec![Step::Field("actor".into())];
        assert!(!t.is_open(&actor));
        t.toggle(&actor);
        assert!(t.is_open(&actor));

        let lines = t.lines("");
        assert_eq!(
            labels(&lines),
            [
                "id",
                "actor",
                "  id",
                "  login",
                "  gravatar_id",
                "payload",
                "public",
                "org"
            ]
        );
        t.toggle(&actor);
        assert_eq!(
            labels(&t.lines("")),
            ["id", "actor", "payload", "public", "org"]
        );
    }

    #[test]
    fn a_list_shows_its_items_by_their_position() {
        let mut t = Tree::new(ROW);
        t.open_path(&[Step::Field("payload".into())]);
        t.open_path(&[
            Step::Field("payload".into()),
            Step::Field("commits".into()),
        ]);
        let lines = t.lines("");
        let found = labels(&lines);
        assert!(found.contains(&"    [0]".to_string()), "{found:?}");
        assert!(found.contains(&"    [1]".to_string()), "{found:?}");
    }

    #[test]
    fn a_structure_says_how_many_fields_it_hides() {
        let t = Tree::new(ROW);
        let line = t
            .lines("")
            .into_iter()
            .find(|l| l.label == "payload")
            .unwrap();
        // The payload holds four fields, and two of them hold a value.
        assert_eq!(line.value, "{2 of 4 fields}");
    }

    #[test]
    fn showing_the_empty_fields_changes_the_short_form_of_a_structure() {
        let mut t = Tree::new(ROW);
        t.hide_empty = false;
        let line = t
            .lines("")
            .into_iter()
            .find(|l| l.label == "payload")
            .unwrap();
        assert_eq!(line.value, "{4 fields}");
    }

    #[test]
    fn a_null_and_an_empty_text_are_not_the_same() {
        let mut t = Tree::new(r#"{"a": null, "b": "", "c": "x"}"#);
        t.hide_empty = false;
        let lines = t.lines("");
        let by = |n: &str| lines.iter().find(|l| l.label == n).unwrap().clone();
        assert_eq!((by("a").value.as_str(), by("a").family), ("NULL", Family::Null));
        assert_eq!((by("b").value.as_str(), by("b").family), ("(empty)", Family::Empty));
        assert_eq!((by("c").value.as_str(), by("c").family), ("x", Family::Text));
    }

    #[test]
    fn the_find_box_opens_the_way_to_a_match() {
        let t = Tree::new(ROW);
        // `sha` is three levels down, and each line above it must stay so
        // that the user sees the way to it.
        let found = labels(&t.lines("sha"));
        assert_eq!(
            found,
            ["payload", "  commits", "    [0]", "      sha", "    [1]", "      sha"]
        );
    }

    #[test]
    fn the_find_box_also_looks_at_the_values() {
        let t = Tree::new(ROW);
        let found = labels(&t.lines("petroav"));
        assert_eq!(found, ["actor", "  login"]);
    }

    #[test]
    fn a_find_with_no_match_gives_no_line() {
        assert!(Tree::new(ROW).lines("zzzz").is_empty());
    }

    #[test]
    fn expand_all_opens_each_level_and_collapse_all_closes_them() {
        let mut t = Tree::new(ROW);
        t.expand_all();
        let found = labels(&t.lines(""));
        assert!(found.contains(&"      sha".to_string()), "{found:?}");
        t.collapse_all();
        assert_eq!(found_depth_max(&t), 0);
    }

    fn found_depth_max(t: &Tree) -> usize {
        t.lines("").iter().map(|l| l.depth).max().unwrap_or(0)
    }

    #[test]
    fn a_value_comes_back_by_its_path() {
        let t = Tree::new(ROW);
        let p = vec![
            Step::Field("payload".into()),
            Step::Field("commits".into()),
            Step::Index(1),
            Step::Field("sha".into()),
        ];
        assert_eq!(t.value_at(&p), Some("bb".to_string()));
        assert_eq!(path_text(&p), "payload.commits[1].sha");
        // A structure comes back as JSON that a person can read.
        let a = t.value_at(&[Step::Field("actor".into())]).unwrap();
        assert!(a.contains("\"login\""), "{a}");
        assert!(a.contains('\n'), "a structure must have line ends");
    }

    #[test]
    fn a_path_that_no_value_holds_gives_nothing() {
        let t = Tree::new(ROW);
        assert_eq!(t.value_at(&[Step::Field("nope".into())]), None);
        assert_eq!(
            t.value_at(&[Step::Field("actor".into()), Step::Index(9)]),
            None
        );
    }

    #[test]
    fn a_row_that_is_not_json_gives_a_message_and_no_line() {
        let t = Tree::new("{not json");
        assert!(t.is_empty());
        assert!(t.error.is_some());
        assert!(t.lines("").is_empty());
    }

    #[test]
    fn an_empty_structure_and_an_empty_list_do_not_open() {
        let mut t = Tree::new(r#"{"a": {}, "b": []}"#);
        t.hide_empty = false;
        for l in t.lines("") {
            assert!(!l.kind.opens(), "{} must not open", l.label);
        }
    }

    #[test]
    fn a_very_deep_value_does_not_fill_the_memory() {
        // Build a value that is deeper than the limit.
        let mut json = String::new();
        for _ in 0..200 {
            json.push_str("{\"a\":");
        }
        json.push('1');
        for _ in 0..200 {
            json.push('}');
        }
        let mut t = Tree::new(&json);
        t.expand_all();
        let lines = t.lines("");
        assert!(
            lines.iter().all(|l| l.depth < MAX_DEPTH),
            "the tree went past the limit"
        );
    }
}
