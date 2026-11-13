//! In-memory source fixtures verify provenance and cursor boundaries without AI/network calls.
use super::parser::{self, Request};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::io::{Cursor, Write};

fn request(media: &str, bytes: &[u8]) -> Request {
    Request {
        media_type: media.into(),
        data: STANDARD.encode(bytes),
        ..Default::default()
    }
}
fn string_cell(column: &str, row: u32, value: &str) -> String {
    format!("<c r=\"{column}{row}\" t=\"inlineStr\"><is><t>{value}</t></is></c>")
}
fn table_row(row: u32, name: &str, price: &str) -> String {
    format!(
        "<row r=\"{row}\">{}{}</row>",
        string_cell("A", row, name),
        string_cell("B", row, price)
    )
}
fn workbook(sheets: &[(&str, String)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut add = |name: &str, content: &str| {
        zip.start_file(name, options).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    };
    let mut types = String::from(
        "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>",
    );
    for i in 1..=sheets.len() {
        types.push_str(&format!("<Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"));
    }
    types.push_str("<Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/></Types>");
    add("[Content_Types].xml", &types);
    add(
        "_rels/.rels",
        "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>",
    );
    let mut book = String::from(
        "<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>",
    );
    let mut rels = String::from(
        "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    for (index, (name, rows)) in sheets.iter().enumerate() {
        let i = index + 1;
        book.push_str(&format!(
            "<sheet name=\"{name}\" sheetId=\"{i}\" r:id=\"rId{i}\"/>"
        ));
        rels.push_str(&format!("<Relationship Id=\"rId{i}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{i}.xml\"/>"));
        add(
            &format!("xl/worksheets/sheet{i}.xml"),
            &format!(
                "<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{rows}</sheetData></worksheet>"
            ),
        );
    }
    add(
        "xl/styles.xml",
        "<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><numFmts count=\"1\"><numFmt numFmtId=\"164\" formatCode=\"000000\"/></numFmts><cellXfs count=\"2\"><xf numFmtId=\"0\"/><xf numFmtId=\"164\"/></cellXfs></styleSheet>",
    );
    rels.push_str("<Relationship Id=\"rIdStyles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>");
    book.push_str("</sheets></workbook>");
    rels.push_str("</Relationships>");
    add("xl/workbook.xml", &book);
    add("xl/_rels/workbook.xml.rels", &rels);
    zip.finish().unwrap().into_inner()
}
#[test]
fn xlsx_styled_empty_rows_cannot_hide_later_products_or_finish_the_sheet() {
    let mut rows = table_row(1, "name", "price");
    for line in 2..504 {
        rows.push_str(&format!(
            "<row r=\"{line}\"><c r=\"A{line}\" s=\"1\"><v></v></c></row>"
        ));
    }
    for line in 504..1104 {
        rows.push_str(&table_row(line, &format!("Product{line}"), "1.25"));
    }
    let bytes = workbook(&[("Products", rows)]);
    let mut req = request(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        &bytes,
    );
    req.header_row = Some(1);
    let first = parser::parse(&req).unwrap();
    assert_eq!(first.rows.len(), 500);
    assert!(!first.done);
    assert_eq!(first.next_sheet, 0);
    assert_eq!(first.columns, ["name", "price"]);
    assert_eq!(first.rows.first().unwrap().line, 504);
    assert_eq!(first.after, 1003);
    req.after = first.after;
    let second = parser::parse(&req).unwrap();
    assert_eq!(second.rows.len(), 100);
    assert!(second.done);
    assert_eq!(second.next_sheet, 1);
    assert_eq!(second.after, 1103);
    let lines: Vec<_> = first
        .rows
        .iter()
        .chain(&second.rows)
        .map(|r| r.line)
        .collect();
    assert_eq!(lines, (504..1104).collect::<Vec<_>>());
}
#[test]
fn xlsx_sheet_transition_keeps_cached_formulas_and_literal_codes() {
    let first = table_row(1, "name", "price")
        + "<row r=\"2\"><c r=\"A2\" t=\"inlineStr\"><is><t>Tea</t></is></c><c r=\"B2\"><f>1+2</f><v>3</v></c></row>";
    let second = table_row(1, "name", "barcode") + &table_row(2, "Coffee", "000123");
    let bytes = workbook(&[("First", first), ("Second", second)]);
    let mut req = request(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        &bytes,
    );
    req.header_row = Some(1);
    let first = parser::parse(&req).unwrap();
    assert_eq!(first.sheet.as_deref(), Some("First"));
    assert!(!first.done);
    assert_eq!(first.next_sheet, 1);
    assert_eq!(first.rows[0].cells, ["Tea", "3"]);
    assert_eq!(first.rows[0].formula_columns, [1]);
    req.sheet_index = first.next_sheet;
    req.after = 0;
    req.header_row = Some(1);
    let second = parser::parse(&req).unwrap();
    assert_eq!(second.sheet.as_deref(), Some("Second"));
    assert!(second.done);
    assert_eq!(second.next_sheet, 2);
    assert_eq!(second.columns, ["name", "barcode"]);
    assert_eq!(second.rows[0].cells, ["Coffee", "000123"]);
    assert_eq!(second.rows[0].line, 2);
}
#[test]
fn uncached_formula_only_rows_remain_visible_for_review() {
    let rows = table_row(1, "name", "price")
        + "<row r=\"2\"><c r=\"A2\"><f>CONCAT(\"Tea\",\" leaves\")</f></c></row>";
    let bytes = workbook(&[("Products", rows)]);
    let mut req = request(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        &bytes,
    );
    req.header_row = Some(1);
    let out = parser::parse(&req).unwrap();
    assert_eq!(
        out.rows.len(),
        1,
        "uncached formulas must be corrected or excluded explicitly"
    );
    assert_eq!(out.rows[0].line, 2);
    assert_eq!(out.rows[0].formula_columns, [0]);
}
#[test]
fn csv_cursor_tracks_physical_record_start_after_multiline_values() {
    let mut req = request("text/csv", b"name;price\n\"Two\nlines\";12\nSingle;3\n");
    req.header_row = Some(1);
    let first = parser::parse(&req).unwrap();
    assert_eq!(
        first.rows.iter().map(|r| r.line).collect::<Vec<_>>(),
        [2, 4]
    );
    assert_eq!(first.rows[0].cells, ["Two\nlines", "12"]);
    req.after = 2;
    let resumed = parser::parse(&req).unwrap();
    assert_eq!(resumed.rows.len(), 1);
    assert_eq!(resumed.rows[0].line, 4);
}
#[test]
fn csv_delimiter_detection_ignores_quoted_punctuation() {
    let req = request("text/csv", b"\"Name, extra, commas\";price\nTea;12.50\n");
    let out = parser::parse(&req).unwrap();
    assert_eq!(out.rows[0].cells, ["Name, extra, commas", "price"]);
    assert_eq!(out.rows[1].cells, ["Tea", "12.50"]);
}
fn pdf() -> Vec<u8> {
    use lopdf::{
        Document, Object, Stream,
        content::{Content, Operation},
        dictionary,
    };
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font =
        doc.add_object(dictionary! {"Type"=>"Font","Subtype"=>"Type1","BaseFont"=>"Courier"});
    let resources = doc.add_object(dictionary! {"Font"=>dictionary!{"F1"=>font}});
    let mut kids = Vec::new();
    for n in 1..=12 {
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), 12.into()]),
                Operation::new("Tj", vec![Object::string_literal(format!("PAGE-{n}"))]),
                Operation::new("ET", vec![]),
            ],
        }
        .encode()
        .unwrap();
        let stream = doc.add_object(Stream::new(dictionary! {}, content));
        let page=doc.add_object(dictionary!{"Type"=>"Page","Parent"=>pages_id,"Contents"=>stream,"Resources"=>resources,"MediaBox"=>vec![0.into(),0.into(),300.into(),300.into()]});
        kids.push(Object::Reference(page));
    }
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {"Type"=>"Pages","Kids"=>kids,"Count"=>12}),
    );
    let catalog = doc.add_object(dictionary! {"Type"=>"Catalog","Pages"=>pages_id});
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    bytes
}
fn excerpt_markers(base64: &str) -> Vec<String> {
    let bytes = STANDARD.decode(base64).unwrap();
    let doc = lopdf::Document::load_mem(&bytes).unwrap();
    doc.get_pages()
        .keys()
        .map(|page| doc.extract_text(&[*page]).unwrap().trim().to_owned())
        .collect()
}
#[test]
fn pdf_context_retains_original_primary_bounds_and_ordered_page_markers() {
    let mut req = request("application/pdf", &pdf());
    req.pdf_from = 6;
    req.pdf_count = 5;
    let out = parser::parse(&req).unwrap();
    assert_eq!((out.page_from, out.page_count, out.total_pages), (6, 5, 12));
    assert!(!out.done);
    assert_eq!(
        excerpt_markers(out.pdf.as_deref().unwrap()),
        (5..=11).map(|n| format!("PAGE-{n}")).collect::<Vec<_>>()
    );
    req.pdf_from = 11;
    let last = parser::parse(&req).unwrap();
    assert_eq!(
        (last.page_from, last.page_count, last.total_pages),
        (11, 2, 12)
    );
    assert!(last.done);
    assert_eq!(
        excerpt_markers(last.pdf.as_deref().unwrap()),
        (10..=12).map(|n| format!("PAGE-{n}")).collect::<Vec<_>>()
    );
}

#[tokio::test]
#[ignore = "Requires GENESY_PRODUCT_PARSER_BIN pointing at the built API executable"]
async fn parser_child_closes_input_and_recovers_after_invalid_source() {
    assert!(std::env::var_os("GENESY_PRODUCT_PARSER_BIN").is_some());
    let invalid = request(
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        b"PK\x03\x04invalid workbook",
    );
    let failure = tokio::time::timeout(std::time::Duration::from_secs(5), parser::read(invalid))
        .await
        .expect("parser must receive request EOF without waiting for its wall-time limit");
    assert!(failure.is_err());
    let mut valid = request("text/csv", b"name,barcode\nTea,000123\n");
    valid.header_row = Some(1);
    let output = tokio::time::timeout(std::time::Duration::from_secs(5), parser::read(valid))
        .await
        .expect("a failed child must release the parser slot")
        .expect("subsequent valid source must parse");
    assert_eq!(output.rows.len(), 1);
    assert_eq!(output.rows[0].cells, ["Tea", "000123"]);
}

#[test]
fn numeric_identifier_padding_is_preserved_without_rounding() {
    for (raw, expected) in [
        ("123", Some("000123")),
        ("1234567", Some("1234567")),
        ("12.5", None),
    ] {
        let rows = table_row(1, "name", "barcode")
            + &format!(
                "<row r=\"2\">{}<c r=\"B2\" s=\"1\"><v>{raw}</v></c></row>",
                string_cell("A", 2, "Tea")
            );
        let bytes = workbook(&[("Products", rows)]);
        let mut req = request(
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            &bytes,
        );
        req.header_row = Some(1);
        let result = parser::parse(&req);
        if let Some(expected) = expected {
            assert_eq!(result.unwrap().rows[0].cells[1], expected);
        } else {
            assert!(matches!(result, Err(message) if message.contains("rounding")));
        }
    }
}
