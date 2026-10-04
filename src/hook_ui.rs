//! Presentation-only adapters for the fixed-purpose, user-initiated Hook capture.
//! Source bytes and cells stay structured; this module does not contact targets.
#![cfg_attr(not(windows), allow(dead_code))]
use coralspy_hook_client::{
    Architecture, CaptureData, CaptureError, CaptureResult, ErrorCode, Operation,
    RECORD_FLAG_CHECKED, RECORD_FLAG_DEFAULT, RECORD_FLAG_DEPTH_LIMIT, RECORD_FLAG_DISABLED,
    RECORD_FLAG_EXPANDED, RECORD_FLAG_HAS_CHILDREN, RECORD_FLAG_NODE_LIMIT, RECORD_FLAG_OWNER_DRAW,
    RECORD_FLAG_SELECTED, RECORD_FLAG_SEPARATOR, RECORD_FLAG_TEXT_TRUNCATED,
};
use coralspynext::model::WindowInfo;

pub struct Snapshot {
    pub operation: Operation,
    pub architecture: Architecture,
    pub result: CaptureResult,
    pub source: Option<WindowInfo>,
}
pub fn operation_for_tab(tab: usize) -> Option<Operation> {
    match tab {
        1 => Some(Operation::ListView),
        2 => Some(Operation::TreeView),
        5 => Some(Operation::RichEditRtf),
        6 => Some(Operation::MenuTarget),
        _ => None,
    }
}
pub fn operation_tab(operation: Operation) -> usize {
    match operation {
        Operation::ListView => 1,
        Operation::TreeView => 2,
        Operation::RichEditRtf => 5,
        Operation::MenuTarget | Operation::MenuDesktopOnce => 6,
    }
}
pub fn architecture_name(architecture: Architecture) -> &'static str {
    match architecture {
        Architecture::X86 => "x86",
        Architecture::X64 => "x64",
    }
}
pub fn operation_name(operation: Operation, english: bool) -> &'static str {
    match (operation, english) {
        (Operation::RichEditRtf, false) => "RichEdit 原始 RTF",
        (Operation::RichEditRtf, true) => "RichEdit original RTF",
        (Operation::ListView, _) => "ListView",
        (Operation::TreeView, _) => "TreeView",
        (Operation::MenuTarget, false) => "所选线程菜单",
        (Operation::MenuTarget, true) => "Selected-thread menu",
        (Operation::MenuDesktopOnce, false) => "桌面菜单（仅一次）",
        (Operation::MenuDesktopOnce, true) => "Desktop menu (one shot)",
    }
}
fn tr<'a>(english: bool, zh: &'a str, en: &'a str) -> &'a str {
    if english {
        en
    } else {
        zh
    }
}
pub fn error_text(error: &CaptureError, english: bool) -> String {
    let (zh, en) = match error.code {
        ErrorCode::InvalidInput | ErrorCode::InvalidRequest => ("Hook 请求无效，请重新选取目标并确认。", "Invalid Hook request. Select the target and confirm again."),
        ErrorCode::TargetMismatch => ("目标已关闭或身份改变，结果已丢弃。请重新选取。", "The target closed or changed identity. Results were discarded. Select it again."),
        ErrorCode::ClassMismatch => ("所选窗口不是此操作支持的原生控件。请选取控件本身。", "The selected window is not a supported native control for this operation. Select the control itself."),
        ErrorCode::PasswordControl => ("密码或受保护控件已拒绝读取。", "Password or protected controls are not read."),
        ErrorCode::AccessDenied | ErrorCode::IntegrityMismatch => ("目标权限或完整性级别不允许读取；没有提升权限。", "The target permissions or integrity level do not allow capture. Privileges were not elevated."),
        ErrorCode::WrongArchitecture => ("目标架构不匹配或不受支持。没有尝试其他目标。", "The target architecture does not match or is unsupported. No other target was tried."),
        ErrorCode::TimedOut => ("本次 Hook 已超时并停止。菜单操作需在时限内打开菜单。", "This Hook capture timed out and stopped. For menu capture, open the menu before the deadline."),
        ErrorCode::Cancelled => ("本次 Hook 已取消。", "This Hook capture was cancelled."),
        ErrorCode::Unsupported => ("此环境或控件不支持该 Hook 操作。", "This environment or control does not support this Hook operation."),
        ErrorCode::ControlError => ("控件未能完整返回内容；没有生成完整导出。", "The control could not return complete content. No complete export was produced."),
        ErrorCode::Ipc => ("Hook 通信失败，结果不可用。", "Hook communication failed. Results are unavailable."),
        ErrorCode::MissingBroker => ("缺少匹配的固定 Hook 组件。请使用完整安装包。", "The matching fixed Hook component is missing. Use the complete package."),
        ErrorCode::OsError => ("Windows 未能完成本次 Hook 操作。", "Windows could not complete this Hook operation."),
    };
    let mut message = tr(english, zh, en).to_string();
    if let Some(code) = error.win32_error {
        message.push_str(&format!(" (Win32 {code})"));
    }
    message
}
impl Snapshot {
    pub fn applies_to(&self, tab: usize) -> bool {
        operation_tab(self.operation) == tab
    }
    /// Rows keep source depth. List rows are logical rows, never individual cells.
    pub fn rows(&self, english: bool) -> Vec<(usize, String, String, String)> {
        match &self.result.data {
            CaptureData::ListView { .. } => self
                .list_cells()
                .into_iter()
                .enumerate()
                .map(|(i, cells)| {
                    (
                        0,
                        cells.first().cloned().unwrap_or_default(),
                        format!("{} {}", tr(english, "行", "Row"), i + 1),
                        cells.into_iter().skip(1).collect::<Vec<_>>().join("\t"),
                    )
                })
                .collect(),
            CaptureData::TreeView { nodes } => nodes
                .iter()
                .map(|node| {
                    (
                        node.depth as usize,
                        node.text.clone(),
                        format!(
                            "{} {}",
                            tr(english, "序号", "Index"),
                            node.id.saturating_add(1)
                        ),
                        flags_label(node.flags, english),
                    )
                })
                .collect(),
            CaptureData::Menu { items, .. } => items
                .iter()
                .map(|item| {
                    (
                        item.depth as usize,
                        if item.flags & RECORD_FLAG_SEPARATOR != 0 && item.text.is_empty() {
                            "────────".into()
                        } else {
                            item.text.clone()
                        },
                        format!("ID {}", item.id),
                        flags_label(item.flags, english),
                    )
                })
                .collect(),
            CaptureData::RichEditRtf { .. } => Vec::new(),
        }
    }
    pub fn list_cells(&self) -> Vec<Vec<String>> {
        let CaptureData::ListView { cells, .. } = &self.result.data else {
            return Vec::new();
        };
        let rows = cells
            .iter()
            .map(|c| c.row.saturating_add(1))
            .max()
            .unwrap_or(0)
            .min(512) as usize;
        let columns = self.list_column_count();
        // Truncation can stop midway through a row. An absent cell must not look
        // like a captured empty value; actual empty records overwrite this marker.
        let mut output = vec![vec!["[未捕获 / not captured]".to_string(); columns]; rows];
        for cell in cells {
            if let Some(value) = output
                .get_mut(cell.row as usize)
                .and_then(|row| row.get_mut(cell.column as usize))
            {
                *value = cell.text.clone();
            }
        }
        output
    }
    fn list_column_count(&self) -> usize {
        let CaptureData::ListView { columns, cells } = &self.result.data else {
            return 0;
        };
        cells
            .iter()
            .map(|cell| cell.column.saturating_add(1))
            .chain(columns.iter().map(|column| column.index.saturating_add(1)))
            .max()
            .unwrap_or(0)
            .min(32) as usize
    }
    /// Preserve actual empty headings; only an absent header record gets an ordinal label.
    pub fn list_headers(&self, english: bool) -> Vec<String> {
        let CaptureData::ListView { columns, .. } = &self.result.data else {
            return Vec::new();
        };
        let mut labels: Vec<String> = (0..self.list_column_count())
            .map(|index| {
                let ordinal = format!("{} {}", tr(english, "列", "Column"), index + 1);
                if columns.is_empty() {
                    ordinal
                } else {
                    format!(
                        "{ordinal} [{}]",
                        tr(english, "未捕获标题", "header not captured")
                    )
                }
            })
            .collect();
        for column in columns {
            if let Some(label) = labels.get_mut(column.index as usize) {
                *label = column.text.clone();
            }
        }
        labels
    }
    pub fn report(&self, english: bool) -> String {
        match &self.result.data {
            CaptureData::RichEditRtf { .. } => String::new(),
            CaptureData::ListView { columns, .. } => {
                // TSV escaping preserves literal tabs/newlines/quotes and empty header/cell positions.
                let mut rows = self.list_cells();
                if !columns.is_empty() {
                    rows.insert(0, self.list_headers(english));
                }
                rows.iter()
                    .map(|row| {
                        row.iter()
                            .map(|cell| tsv_cell(cell))
                            .collect::<Vec<_>>()
                            .join("\t")
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            _ => self
                .rows(english)
                .iter()
                .map(|(depth, name, role, value)| {
                    format!(
                        "{}{}\t{}\t{}",
                        "  ".repeat((*depth).min(32)),
                        name,
                        role,
                        value
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
    pub fn row_report(&self, index: usize, english: bool) -> Option<String> {
        match &self.result.data {
            CaptureData::RichEditRtf { .. } => None,
            CaptureData::ListView { .. } => self.list_cells().get(index).map(|row| {
                row.iter()
                    .map(|cell| tsv_cell(cell))
                    .collect::<Vec<_>>()
                    .join("\t")
            }),
            _ => self
                .rows(english)
                .get(index)
                .map(|(depth, name, role, value)| {
                    format!(
                        "{}{}\t{}\t{}",
                        "  ".repeat((*depth).min(32)),
                        name,
                        role,
                        value
                    )
                }),
        }
    }
    pub fn export_report(&self, english: bool) -> String {
        let target = &self.result.actual_target;
        let warning = if self.result.truncated {
            tr(
                english,
                "警告：已截断，内容不完整。",
                "WARNING: truncated; content is incomplete.",
            )
        } else {
            tr(
                english,
                "本次已捕获的快照；目标内容可能随后变化。",
                "Snapshot captured by this operation; target content may change afterward.",
            )
        };
        format!(
            "CoralSpyNext Hook / {} / HWND 0x{:016X} / PID {} / TID {}\n{}\n{}\n\n{}",
            architecture_name(self.architecture),
            target.hwnd,
            target.pid,
            target.tid,
            self.counts(english),
            warning,
            self.report(english)
        )
    }
    pub fn counts(&self, english: bool) -> String {
        let c = &self.result.reported_counts;
        let unknown = tr(english, "未提供", "not reported");
        let count = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_else(|| unknown.into());
        match &self.result.data {
            CaptureData::ListView { columns, cells } => format!(
                "{}: {} × {}; {}: {} × {}; {}: {}; {}: {}",
                tr(english, "已捕获行/列", "Captured rows/columns"),
                self.list_cells().len(),
                self.list_column_count(),
                tr(english, "来源行/列", "Source rows/columns"),
                count(c.rows),
                count(c.columns),
                tr(english, "单元格", "Cells"),
                cells.len(),
                tr(english, "列标题", "Headers"),
                columns.len()
            ),
            CaptureData::TreeView { nodes } => format!(
                "{}: {}; {}: {}",
                tr(english, "已捕获节点", "Captured nodes"),
                nodes.len(),
                tr(english, "来源节点", "Source nodes"),
                count(c.nodes)
            ),
            CaptureData::Menu { items, .. } => format!(
                "{}: {}; {}: {}",
                tr(english, "已捕获菜单项", "Captured menu items"),
                items.len(),
                tr(english, "来源项目", "Source items"),
                count(c.nodes)
            ),
            CaptureData::RichEditRtf { bytes, .. } => format!(
                "{}: {} bytes",
                tr(english, "原始 RTF", "Original RTF"),
                bytes.len()
            ),
        }
    }
}
fn flags_label(flags: u32, english: bool) -> String {
    let labels = [
        (RECORD_FLAG_DISABLED, "禁用", "Disabled"),
        (RECORD_FLAG_CHECKED, "已勾选", "Checked"),
        (RECORD_FLAG_DEFAULT, "默认", "Default"),
        (RECORD_FLAG_HAS_CHILDREN, "含子项", "Has children"),
        (RECORD_FLAG_SEPARATOR, "分隔符", "Separator"),
        (RECORD_FLAG_OWNER_DRAW, "自绘", "Owner-drawn"),
        (RECORD_FLAG_EXPANDED, "已展开", "Expanded"),
        (RECORD_FLAG_SELECTED, "已选中", "Selected"),
        (RECORD_FLAG_TEXT_TRUNCATED, "文字截断", "Text truncated"),
        (RECORD_FLAG_DEPTH_LIMIT, "深度上限", "Depth limit"),
        (RECORD_FLAG_NODE_LIMIT, "节点上限", "Node limit"),
    ];
    labels
        .iter()
        .filter(|(flag, _, _)| flags & flag != 0)
        .map(|(_, zh, en)| tr(english, zh, en))
        .collect::<Vec<_>>()
        .join(" / ")
}
fn tsv_cell(text: &str) -> String {
    if text
        .chars()
        .any(|ch| matches!(ch, '\t' | '\n' | '\r' | '"'))
    {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coralspy_hook_client::{Cell, ReportedCounts, Target};
    fn list(cells: Vec<Cell>) -> Snapshot {
        Snapshot {
            operation: Operation::ListView,
            architecture: Architecture::X86,
            source: None,
            result: CaptureResult {
                actual_target: Target {
                    hwnd: 1,
                    pid: 2,
                    tid: 3,
                },
                truncated: false,
                reported_counts: ReportedCounts::default(),
                captured_records: cells.len() as u32,
                data: CaptureData::ListView {
                    columns: Vec::new(),
                    cells,
                },
            },
        }
    }
    #[test]
    fn list_preserves_empty_cells_and_literal_tabs() {
        let snapshot = list(vec![
            Cell {
                row: 0,
                column: 0,
                flags: 0,
                text: "a\tb".into(),
            },
            Cell {
                row: 0,
                column: 1,
                flags: 0,
                text: String::new(),
            },
            Cell {
                row: 0,
                column: 2,
                flags: 0,
                text: "尾列".into(),
            },
            Cell {
                row: 1,
                column: 0,
                flags: 0,
                text: String::new(),
            },
            Cell {
                row: 1,
                column: 2,
                flags: 0,
                text: String::new(),
            },
            Cell {
                row: 1,
                column: 1,
                flags: 0,
                text: "x\ny".into(),
            },
        ]);
        assert_eq!(
            snapshot.list_cells(),
            vec![vec!["a\tb", "", "尾列"], vec!["", "x\ny", ""]]
        );
        assert_eq!(snapshot.rows(false).len(), 2);
        assert_eq!(snapshot.report(true), "\"a\tb\"\t\t尾列\n\t\"x\ny\"\t");
    }
    #[test]
    fn result_never_crosses_tabs() {
        let snapshot = list(vec![]);
        assert!(snapshot.applies_to(1));
        assert!(!snapshot.applies_to(5));
    }
    #[test]
    fn source_text_is_not_translated() {
        let snapshot = list(vec![Cell {
            row: 0,
            column: 0,
            flags: 0,
            text: "拒绝访问 • User text".into(),
        }]);
        assert_eq!(snapshot.list_cells()[0][0], "拒绝访问 • User text");
        assert_eq!(snapshot.report(true), snapshot.report(false));
    }
    #[test]
    fn export_retains_source_and_truncation() {
        let mut snapshot = list(vec![Cell {
            row: 0,
            column: 0,
            flags: 0,
            text: "User text".into(),
        }]);
        snapshot.result.truncated = true;
        snapshot.result.reported_counts.rows = Some(900);
        let report = snapshot.export_report(true);
        assert!(report.contains("PID 2 / TID 3"));
        assert!(report.contains("Source rows/columns: 900"));
        assert!(report.contains("WARNING: truncated"));
        assert!(report.ends_with("User text"));
    }
    #[test]
    fn current_list_row_preserves_column_order_and_quoting() {
        let snapshot = list(vec![
            Cell {
                row: 0,
                column: 0,
                flags: 0,
                text: String::new(),
            },
            Cell {
                row: 0,
                column: 1,
                flags: 0,
                text: String::new(),
            },
            Cell {
                row: 0,
                column: 2,
                flags: 0,
                text: "a\tb".into(),
            },
        ]);
        assert_eq!(snapshot.row_report(0, true), Some("\t\t\"a\tb\"".into()));
        assert_eq!(snapshot.row_report(1, true), None);
    }
    #[test]
    fn incomplete_rtf_cannot_be_exported() {
        let mut snapshot = list(vec![]);
        snapshot.operation = Operation::RichEditRtf;
        snapshot.result.data = CaptureData::RichEditRtf {
            bytes: vec![123, 92, 114, 116, 102, 255, 125],
            complete: true,
        };
        assert_eq!(
            snapshot.result.complete_rtf_bytes(),
            Some([123, 92, 114, 116, 102, 255, 125].as_slice())
        );
        snapshot.result.truncated = true;
        assert_eq!(snapshot.result.complete_rtf_bytes(), None);
        snapshot.result.truncated = false;
        if let CaptureData::RichEditRtf { complete, .. } = &mut snapshot.result.data {
            *complete = false;
        }
        assert_eq!(snapshot.result.complete_rtf_bytes(), None);
        assert_eq!(snapshot.row_report(0, true), None);
    }
    #[test]
    fn tree_depth_and_menu_flags_remain_typed() {
        let mut snapshot = list(vec![]);
        snapshot.operation = Operation::TreeView;
        snapshot.result.data = CaptureData::TreeView {
            nodes: vec![coralspy_hook_client::TreeNode {
                depth: 3,
                id: 8,
                flags: RECORD_FLAG_EXPANDED | RECORD_FLAG_HAS_CHILDREN,
                text: "用户节点".into(),
            }],
        };
        let rows = snapshot.rows(true);
        assert_eq!(rows[0].0, 3);
        assert_eq!(rows[0].1, "用户节点");
        assert_eq!(rows[0].2, "Index 9");
        assert!(rows[0].3.contains("Expanded"));
        snapshot.operation = Operation::MenuTarget;
        snapshot.result.data = CaptureData::Menu {
            root_menu: 4,
            items: vec![coralspy_hook_client::MenuItem {
                depth: 2,
                id: 77,
                flags: RECORD_FLAG_DISABLED | RECORD_FLAG_CHECKED,
                text: "Open".into(),
            }],
        };
        let rows = snapshot.rows(false);
        assert_eq!(rows[0].0, 2);
        assert_eq!(rows[0].1, "Open");
        assert_eq!(rows[0].2, "ID 77");
        assert!(rows[0].3.contains("禁用"));
        assert!(rows[0].3.contains("已勾选"));
    }
    #[test]
    fn real_headers_preserve_empty_labels_tabs_and_column_positions() {
        let mut snapshot = list(vec![
            Cell {
                row: 0,
                column: 1,
                flags: 0,
                text: String::new(),
            },
            Cell {
                row: 0,
                column: 2,
                flags: 0,
                text: String::new(),
            },
            Cell {
                row: 0,
                column: 0,
                flags: 0,
                text: "value".into(),
            },
        ]);
        if let CaptureData::ListView { columns, .. } = &mut snapshot.result.data {
            *columns = vec![
                coralspy_hook_client::ListColumn {
                    index: 0,
                    flags: 0,
                    text: "名称\tName".into(),
                },
                coralspy_hook_client::ListColumn {
                    index: 1,
                    flags: 0,
                    text: String::new(),
                },
                coralspy_hook_client::ListColumn {
                    index: 2,
                    flags: 0,
                    text: "引号\"列".into(),
                },
            ];
        }
        assert_eq!(
            snapshot.list_headers(true),
            vec!["名称\tName", "", "引号\"列"]
        );
        assert_eq!(snapshot.list_headers(false), snapshot.list_headers(true));
        assert_eq!(snapshot.list_cells(), vec![vec!["value", "", ""]]);
        assert_eq!(snapshot.rows(true).len(), 1);
        assert_eq!(
            snapshot.report(true),
            "\"名称\tName\"\t\t\"引号\"\"列\"\nvalue\t\t"
        );
        assert!(snapshot.counts(true).contains("Cells: 3; Headers: 3"));
        assert_eq!(snapshot.row_report(0, true), Some("value\t\t".into()));
    }
    #[test]
    fn zero_row_list_retains_real_headers_without_inventing_rows() {
        let mut snapshot = list(vec![]);
        if let CaptureData::ListView { columns, .. } = &mut snapshot.result.data {
            *columns = vec![
                coralspy_hook_client::ListColumn {
                    index: 0,
                    flags: 0,
                    text: "Empty list".into(),
                },
                coralspy_hook_client::ListColumn {
                    index: 1,
                    flags: 0,
                    text: String::new(),
                },
            ];
        }
        assert!(snapshot.list_cells().is_empty());
        assert!(snapshot.rows(true).is_empty());
        assert_eq!(snapshot.list_headers(true), vec!["Empty list", ""]);
        assert_eq!(snapshot.report(true), "Empty list\t");
        assert!(snapshot
            .counts(true)
            .contains("Captured rows/columns: 0 × 2"));
    }
    #[test]
    fn absent_header_is_distinct_from_captured_empty_header() {
        let mut snapshot = list(vec![Cell {
            row: 0,
            column: 1,
            flags: 0,
            text: "v".into(),
        }]);
        if let CaptureData::ListView { columns, .. } = &mut snapshot.result.data {
            *columns = vec![coralspy_hook_client::ListColumn {
                index: 1,
                flags: 0,
                text: String::new(),
            }];
        }
        assert_eq!(
            snapshot.list_headers(true),
            vec!["Column 1 [header not captured]", ""]
        );
        assert_eq!(snapshot.list_headers(false), vec!["列 1 [未捕获标题]", ""]);
    }
    #[test]
    fn partial_row_never_masquerades_as_empty_captured_cells() {
        let mut snapshot = list(vec![Cell {
            row: 0,
            column: 0,
            flags: 0,
            text: String::new(),
        }]);
        snapshot.result.truncated = true;
        if let CaptureData::ListView { columns, .. } = &mut snapshot.result.data {
            *columns = vec![
                coralspy_hook_client::ListColumn {
                    index: 0,
                    flags: 0,
                    text: "Name".into(),
                },
                coralspy_hook_client::ListColumn {
                    index: 1,
                    flags: 0,
                    text: "Value".into(),
                },
            ];
        }
        assert_eq!(
            snapshot.list_cells(),
            vec![vec!["", "[未捕获 / not captured]"]]
        );
        assert!(snapshot.export_report(true).contains("WARNING: truncated"));
        assert!(snapshot.report(true).ends_with("\t[未捕获 / not captured]"));
    }
}
