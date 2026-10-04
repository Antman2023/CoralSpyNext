//! Presentation views of captured UIA data. Counts mean exposed logical items,
//! never the number of incidental containers or descendant text fragments.
use crate::model::{ContentNode, ContentSnapshot};
pub type ContentRow = (usize, String, String, String);
fn safe_name(node: &ContentNode) -> String {
    if node.is_password {
        "[Protected]".into()
    } else {
        node.name.clone()
    }
}
fn safe_value(node: &ContentNode) -> String {
    if node.is_password {
        "[Protected]".into()
    } else {
        node.value.clone()
    }
}
fn is_row(node: &ContentNode, tab: usize) -> bool {
    match tab {
        0 => node.role == "ListItem",
        1 => matches!(node.role.as_str(), "DataItem" | "ListItem"),
        2 => node.role == "TreeItem",
        _ => true,
    }
}
pub fn list_headers(snapshot: &ContentSnapshot) -> Vec<String> {
    snapshot
        .nodes
        .iter()
        .filter(|n| n.role == "HeaderItem" && !n.is_password)
        .map(safe_name)
        .collect()
}
pub fn logical_rows(snapshot: &ContentSnapshot, tab: usize) -> Vec<ContentRow> {
    let mut rows = Vec::new();
    let mut tree_ancestors: Vec<usize> = Vec::new();
    let mut row_ancestor: Option<usize> = None;
    for (i, node) in snapshot.nodes.iter().enumerate() {
        while tree_ancestors.last().is_some_and(|d| *d >= node.depth) {
            tree_ancestors.pop();
        }
        if row_ancestor.is_some_and(|d| node.depth <= d) {
            row_ancestor = None;
        }
        if !is_row(node, tab) {
            continue;
        }
        if tab == 1 {
            if row_ancestor.is_some() {
                continue;
            }
            row_ancestor = Some(node.depth);
        }
        let depth = if tab == 2 {
            while tree_ancestors.last().is_some_and(|d| *d >= node.depth) {
                tree_ancestors.pop();
            }
            let d = tree_ancestors.len();
            tree_ancestors.push(node.depth);
            d
        } else {
            0
        };
        let mut value = safe_value(node);
        if tab == 1 && !node.is_password {
            let mut cells = Vec::new();
            for child in snapshot.nodes[i + 1..]
                .iter()
                .take_while(|n| n.depth > node.depth)
            {
                if child.depth == node.depth + 1
                    && matches!(child.role.as_str(), "Text" | "Custom" | "Edit" | "DataItem")
                {
                    let cell = if child.value.is_empty() {
                        safe_name(child)
                    } else {
                        safe_value(child)
                    };
                    cells.push(cell);
                }
            }
            if !cells.is_empty() {
                value = cells.join("\t");
            }
        }
        rows.push((depth, safe_name(node), node.role.clone(), value));
    }
    rows
}
/// Cells are kept structured for display: literal tabs and empty values must
/// never alter the column positions that the accessibility provider exposed.
pub fn list_cells(snapshot: &ContentSnapshot) -> Vec<Vec<String>> {
    let mut output = Vec::new();
    let mut ancestor: Option<usize> = None;
    for (index, node) in snapshot.nodes.iter().enumerate() {
        if ancestor.is_some_and(|depth| node.depth <= depth) {
            ancestor = None;
        }
        if !is_row(node, 1) || ancestor.is_some() {
            continue;
        }
        ancestor = Some(node.depth);
        if node.is_password {
            output.push(vec!["[Protected]".into()]);
            continue;
        }
        let cells = snapshot.nodes[index + 1..]
            .iter()
            .take_while(|n| n.depth > node.depth)
            .filter(|n| {
                n.depth == node.depth + 1
                    && matches!(n.role.as_str(), "Text" | "Custom" | "Edit" | "DataItem")
            })
            .map(|n| {
                if n.value.is_empty() {
                    safe_name(n)
                } else {
                    safe_value(n)
                }
            })
            .collect();
        output.push(cells);
    }
    output
}
/// A TextPattern/ValuePattern payload is in node.value. snapshot.text is a
/// diagnostic tree report and must never masquerade as editor text.
pub fn rich_text(snapshot: &ContentSnapshot) -> String {
    let candidates: Vec<&ContentNode> = snapshot
        .nodes
        .iter()
        .filter(|n| !n.is_password && matches!(n.role.as_str(), "Edit" | "Document"))
        .collect();
    if let Some(node) = candidates
        .iter()
        .find(|n| n.class_name.to_ascii_lowercase().contains("rich") && !n.value.is_empty())
    {
        return node.value.clone();
    }
    if let Some(node) = candidates.iter().find(|n| !n.value.is_empty()) {
        return node.value.clone();
    }
    String::new()
}
pub fn report(snapshot: &ContentSnapshot, tab: usize) -> String {
    let mut out = format!(
        "CoralSpyNext content snapshot\nSource: {}\nHWND: 0x{:016X}\n",
        snapshot.source, snapshot.hwnd
    );
    if snapshot.truncated {
        out.push_str("WARNING: Capture is incomplete/truncated. This is not all data in the source control.\n");
    }
    for warning in &snapshot.warnings {
        out.push_str("WARNING: ");
        out.push_str(warning);
        out.push('\n');
    }
    let rows = logical_rows(snapshot, tab);
    out.push_str(&format!("Exposed logical items: {}\n\n", rows.len()));
    let headers = list_headers(snapshot);
    if tab == 1 && !headers.is_empty() {
        out.push_str(&headers.join("\t"));
        out.push('\n');
    }
    for (depth, name, role, value) in rows {
        out.push_str(&format!(
            "{}{}\t{}\t{}\n",
            "  ".repeat(depth.min(32)),
            name,
            role,
            value
        ));
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    fn n(depth: usize, role: &str, name: &str, value: &str) -> ContentNode {
        ContentNode {
            depth,
            role: role.into(),
            name: name.into(),
            value: value.into(),
            ..Default::default()
        }
    }
    #[test]
    fn list_cells_preserve_empty_and_literal_tab() {
        let s = ContentSnapshot {
            nodes: vec![
                n(0, "DataGrid", "g", ""),
                n(1, "DataItem", "r", ""),
                n(2, "Text", "A", ""),
                n(2, "Text", "", ""),
                n(2, "Text", "C\tD", ""),
            ],
            ..Default::default()
        };
        assert_eq!(list_cells(&s), vec![vec!["A", "", "C\tD"]]);
        assert_eq!(logical_rows(&s, 1)[0].3, "A\t\tC\tD");
    }
    #[test]
    fn blank_headers_preserve_column_position() {
        let s = ContentSnapshot {
            nodes: vec![
                n(0, "HeaderItem", "first", ""),
                n(0, "HeaderItem", "", ""),
                n(0, "HeaderItem", "third", ""),
            ],
            ..Default::default()
        };
        assert_eq!(list_headers(&s), vec!["first", "", "third"]);
    }
    #[test]
    fn distinct_trees_do_not_share_ancestors() {
        let s = ContentSnapshot {
            nodes: vec![
                n(1, "Tree", "first", ""),
                n(2, "TreeItem", "A", ""),
                n(1, "Tree", "second", ""),
                n(2, "Group", "g", ""),
                n(3, "TreeItem", "B", ""),
            ],
            ..Default::default()
        };
        assert_eq!(
            logical_rows(&s, 2).iter().map(|r| r.0).collect::<Vec<_>>(),
            vec![0, 0]
        );
    }
    #[test]
    fn nested_data_cells_are_not_rows() {
        let s = ContentSnapshot {
            nodes: vec![
                n(0, "DataGrid", "g", ""),
                n(1, "DataItem", "row", ""),
                n(2, "DataItem", "cell", "value"),
            ],
            ..Default::default()
        };
        assert_eq!(logical_rows(&s, 1).len(), 1);
    }
    #[test]
    fn logical_count_excludes_containers_and_text() {
        let s = ContentSnapshot {
            nodes: vec![
                n(0, "List", "L", ""),
                n(1, "ListItem", "one", ""),
                n(2, "Text", "fragment", ""),
                n(1, "ListItem", "two", ""),
            ],
            ..Default::default()
        };
        assert_eq!(logical_rows(&s, 0).len(), 2);
    }
    #[test]
    fn tree_depth_is_normalized_to_items() {
        let s = ContentSnapshot {
            nodes: vec![
                n(3, "Tree", "T", ""),
                n(4, "TreeItem", "root", ""),
                n(5, "TreeItem", "child", ""),
                n(4, "TreeItem", "peer", ""),
            ],
            ..Default::default()
        };
        assert_eq!(
            logical_rows(&s, 2).iter().map(|r| r.0).collect::<Vec<_>>(),
            vec![0, 1, 0]
        );
    }
    #[test]
    fn rich_text_is_payload_not_report() {
        let s = ContentSnapshot {
            nodes: vec![n(0, "Edit", "editor", "Hello\n你好")],
            text: "[Edit] editor : Hello".into(),
            ..Default::default()
        };
        assert_eq!(rich_text(&s), "Hello\n你好");
    }
    #[test]
    fn exports_include_warnings() {
        let s = ContentSnapshot {
            truncated: true,
            warnings: vec!["virtualized".into()],
            ..Default::default()
        };
        let r = report(&s, 1);
        assert!(r.contains("incomplete/truncated"));
        assert!(r.contains("virtualized"));
    }
    #[test]
    fn protected_values_never_exported() {
        let mut a = n(0, "ListItem", "secretname", "secretvalue");
        a.is_password = true;
        let r = report(
            &ContentSnapshot {
                nodes: vec![a],
                ..Default::default()
            },
            0,
        );
        assert!(!r.contains("secret"));
    }
    #[test]
    fn rows_preserve_exposed_cells() {
        let s = ContentSnapshot {
            nodes: vec![
                n(0, "DataGrid", "grid", ""),
                n(1, "DataItem", "row", ""),
                n(2, "Text", "A", ""),
                n(2, "Text", "B", ""),
            ],
            ..Default::default()
        };
        assert_eq!(logical_rows(&s, 1)[0].3, "A\tB");
    }
}
