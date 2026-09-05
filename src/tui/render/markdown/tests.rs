use super::*;

fn render(source: &str, width: u16) -> Vec<Row> {
    blocks(source)
        .flat_map(|block| block.unwrap().rows(width))
        .collect()
}

fn text(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| row.iter().map(|s| s.text.as_str()).collect())
        .collect()
}

#[test]
fn blocks_preserve_structure_styles_and_reference_destinations() {
    let source = "# Title\n\n**bold** *italic* ~~gone~~ `code`\n\n[**same**][ref] and [same](https://other.test)\n\n[ref]: https://example.test/a\n";
    assert_eq!(blocks(source).count(), 3);
    let rows = render(source, 80);
    assert_eq!(
        text(&rows),
        ["Title", "bold italic gone code", "same and same"]
    );
    assert!(rows[0][0].style.add_modifier.contains(Modifier::BOLD));
    for (word, modifier) in [
        ("bold", Modifier::BOLD),
        ("italic", Modifier::ITALIC),
        ("gone", Modifier::CROSSED_OUT),
    ] {
        assert!(
            rows[1]
                .iter()
                .any(|s| s.text == word && s.style.add_modifier.contains(modifier))
        );
    }
    let links: Vec<_> = rows[2].iter().filter_map(|s| s.link.as_deref()).collect();
    assert_eq!(links, ["https://example.test/a", "https://other.test"]);
    assert!(rows[2][0].style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn wraps_unicode_and_link_labels_without_losing_content_or_target() {
    let source = "[中文e\u{301}👩‍💻结束](https://example.test)";
    let rows = render(source, 5);
    assert_eq!(text(&rows).concat(), "中文e\u{301}👩‍💻结束");
    for row in &rows {
        assert!(row.iter().map(|s| s.text.width()).sum::<usize>() <= 5);
        assert!(
            row.iter()
                .all(|s| s.link.as_deref() == Some("https://example.test"))
        );
    }
    let narrow = text(&render("中", 1)).concat();
    assert_eq!(narrow, "\\u{4e2d}");
    assert_eq!(text(&render("**e**\u{301}后", 2)), ["e\u{301}", "后"]);
}

#[test]
fn lists_quotes_and_code_keep_hanging_indents() {
    assert_eq!(
        text(&render("- abcdefghij\n- [x] done", 8)),
        ["• abcdef", "  ghij", "• [x] do", "  ne"]
    );
    assert_eq!(
        text(&render("> quote\n>\n> 3. outer\n>    - inner", 80)),
        ["│ quote", "│ 3. outer", "│    • inner"]
    );
    assert_eq!(
        text(&render("- first\n\n  second", 80)),
        ["• first", "  second"]
    );
}

#[test]
fn code_highlighting_preserves_blank_lines_indentation_and_unclosed_fences() {
    let rows = render("```rust\nfn main() {\n\n    println!(\"hi\");\n}", 80);
    assert_eq!(
        text(&rows),
        [
            "```rust",
            "fn main() {",
            "",
            "    println!(\"hi\");",
            "}",
            "```"
        ]
    );
    let colors: std::collections::HashSet<_> =
        rows.iter().flatten().filter_map(|s| s.style.fg).collect();
    assert!(colors.len() >= 3, "code must have distinct token colors");
    assert_eq!(
        text(&render("```unknown-xyz\n  x\n\n```", 80)),
        ["```unknown-xyz", "  x", "", "```"]
    );
}

#[test]
fn tables_align_cells_preserve_links_and_clip_only_at_the_right_edge() {
    let source =
        "| Key | Value |\n| :-- | --: |\n| [中](https://example.test) | 12345 |\n\nAfter the table";
    let rows = render(source, 80);
    assert_eq!(
        text(&rows),
        [
            "│ Key │ Value │",
            "├─────┼───────┤",
            "│ 中  │ 12345 │",
            "After the table"
        ]
    );
    assert!(rows[2].iter().any(|s| s.text == "中" && s.link.is_some()));
    assert_eq!(
        text(&render(source, 7)),
        ["│ Key │", "├─────┼", "│ 中  │", "After t", "he tabl", "e"]
    );
    assert_eq!(text(&render("|中x|\n|--|\n|值|", 3)), ["│ ", "├──", "│ "]);
}

#[test]
fn controls_are_sanitized_after_markdown_entities_are_decoded() {
    let rows = render(
        "Hi &#27;[2J\u{1b}]0;title\u{7} **bold** ![alt](image.png)",
        200,
    );
    let text = text(&rows).concat();
    assert!(text.contains("\\u{1b}[2J\\u{1b}]0;title\\u{7}"));
    assert!(text.contains("bold [img] alt"));
    assert!(!text.chars().any(char::is_control));
}

#[test]
fn large_answer_is_not_limited_to_u16_rows_or_terminal_height() {
    let source = format!("```\n{}```", "x\n".repeat(65_536));
    let rows = render(&source, 1);
    assert_eq!(
        rows.iter()
            .filter(|row| row.iter().any(|s| s.text == "x"))
            .count(),
        65_536
    );
}
