//! MIME edge cases for IMAP accounts: real-world and hostile messages must
//! parse without panics, show the right text and attachments, and outgoing
//! messages must never let user input create extra headers.

use base64::Engine;
use mailcore::imap::mime::{self, Outgoing};
use mailcore::model::{Address, MessageDetail, MessageSummary};

fn summary(raw: &str) -> MessageSummary {
    mime::summary("mb", "INBOX", "1:1:INBOX".into(), raw.as_bytes(), None, &[], None)
}

fn detail(raw: &str) -> MessageDetail {
    mime::detail(raw.as_bytes(), summary(raw)).expect("detail")
}

fn addr(n: &str, a: &str) -> Address {
    Address {
        name: n.into(),
        address: a.into(),
    }
}

fn outgoing() -> Outgoing {
    Outgoing {
        from: addr("Anna", "anna@example.org"),
        to: vec![addr("Bernd", "bernd@example.com")],
        subject: "Test".into(),
        html: "<p>Hallo</p>".into(),
        importance: "normal".into(),
        message_id: mime::new_message_id("anna@example.org"),
        ..Default::default()
    }
}

/// Header block of a formatted message (up to the first empty line).
fn headers(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw).into_owned();
    s.split("\r\n\r\n").next().unwrap_or("").to_string()
}

/// Header names at the start of (unfolded) header lines.
fn header_names(raw: &[u8]) -> Vec<String> {
    headers(raw)
        .split("\r\n")
        .filter(|l| !l.starts_with(' ') && !l.starts_with('\t'))
        .filter_map(|l| l.split_once(':').map(|(n, _)| n.to_ascii_lowercase()))
        .collect()
}

// ---- structure ----

#[test]
fn nested_mixed_alternative_related() {
    let png = base64::engine::general_purpose::STANDARD.encode([137u8, 80, 78, 71, 1, 2, 3]);
    let raw = format!(
        "From: a@example.com\r\nSubject: Verschachtelt\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"m\"\r\n\r\n\
--m\r\nContent-Type: multipart/alternative; boundary=\"a\"\r\n\r\n\
--a\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nNur Text\r\n\
--a\r\nContent-Type: multipart/related; boundary=\"r\"\r\n\r\n\
--r\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>HTML <img src=\"cid:bild1\"></p>\r\n\
--r\r\nContent-Type: image/png\r\nContent-ID: <bild1>\r\nContent-Transfer-Encoding: base64\r\n\r\n{png}\r\n\
--r--\r\n--a--\r\n\
--m\r\nContent-Type: application/pdf; name=\"doc.pdf\"\r\nContent-Disposition: attachment; filename=\"doc.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0=\r\n\
--m--\r\n"
    );
    let d = detail(&raw);
    assert!(d.body_html.contains("HTML"), "{}", d.body_html);
    assert!(!d.body_html.contains("Nur Text"));
    assert!(d.body_html.contains("data:image/png;base64,"), "{}", d.body_html);
    assert_eq!(d.attachments.len(), 1, "{:?}", d.attachments);
    assert_eq!(d.attachments[0].name, "doc.pdf");
    assert_eq!(d.attachments[0].content_type, "application/pdf");
    assert!(d.summary.has_attachments);
    let (name, _, bytes) = mime::attachment(raw.as_bytes(), d.attachments[0].id.parse().unwrap()).unwrap();
    assert_eq!((name.as_str(), bytes.as_slice()), ("doc.pdf", b"%PDF-".as_slice()));
}

#[test]
fn missing_and_odd_boundaries_do_not_panic() {
    let cases = [
        // Boundary declared but never used.
        "Subject: a\r\nContent-Type: multipart/mixed; boundary=\"nie\"\r\n\r\nText ohne Teile\r\n",
        // No boundary parameter at all.
        "Subject: b\r\nContent-Type: multipart/mixed\r\n\r\n--x\r\nContent-Type: text/plain\r\n\r\nhallo\r\n--x--\r\n",
        // Missing closing boundary.
        "Subject: c\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nhallo ohne ende\r\n",
        // Empty boundary.
        "Subject: d\r\nContent-Type: multipart/alternative; boundary=\"\"\r\n\r\n--\r\n\r\nx\r\n----\r\n",
        // Boundary with special characters and nested reuse of the same boundary.
        "Subject: e\r\nContent-Type: multipart/mixed; boundary=\"=_a'()+,-./:=?\"\r\n\r\n--=_a'()+,-./:=?\r\nContent-Type: multipart/mixed; boundary=\"=_a'()+,-./:=?\"\r\n\r\n--=_a'()+,-./:=?\r\n\r\ninnen\r\n--=_a'()+,-./:=?--\r\n",
        // Only headers.
        "Subject: f\r\n",
        // Header line without colon and binary garbage.
        "Subject: g\r\nDas ist kein Header\r\n\r\n\u{0}\u{1}\u{2}\u{ff}",
    ];
    for raw in cases {
        let s = summary(raw);
        let _ = mime::detail(raw.as_bytes(), s);
        let _ = mime::all_attachments(raw.as_bytes());
    }
    let d = detail(cases[2]);
    assert!(d.body_html.contains("hallo ohne ende"), "{}", d.body_html);
}

#[test]
fn empty_and_garbage_messages() {
    for raw in [&b""[..], b"\r\n", b"\r\n\r\n", b"\xff\xfe\x00", &[b'a'; 10][..]] {
        let s = mime::summary("mb", "INBOX", "1:1:INBOX".into(), raw, None, &[], None);
        assert!(s.received_at.is_empty() || !s.received_at.is_empty());
        let _ = mime::detail(raw, s);
        assert!(mime::all_attachments(raw).is_empty() || raw.len() > 3);
        assert!(mime::attachment(raw, 0).is_err());
    }
}

#[test]
fn no_content_type_is_plain_text() {
    let d = detail("From: a@example.com\r\nSubject: Ohne Typ\r\n\r\nZeile 1\r\nZeile 2 <b>kein html</b>\r\n");
    assert!(d.body_html.contains("Zeile 1"));
    assert!(d.body_html.contains("&lt;b&gt;kein html&lt;/b&gt;"), "{}", d.body_html);
    assert!(d.attachments.is_empty());
}

// ---- plain text must be escaped ----

#[test]
fn plain_text_is_escaped_and_keeps_layout() {
    let raw = "From: a@example.com\r\nSubject: Text\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n\
<script>alert(1)</script>\r\n<img src=x onerror=alert(2)>\r\nTom &amp; Jerry & \"Quotes\"\r\n    eingerueckt\r\n";
    let d = detail(raw);
    let html = &d.body_html;
    assert!(!html.contains("<script"), "{html}");
    assert!(!html.contains("<img"), "{html}");
    // The literal text "&amp;" must be shown as typed, i.e. escaped once more.
    assert!(html.contains("Tom &amp;amp; Jerry &amp;"), "{html}");
    // Whitespace layout is preserved (pre-wrap block).
    assert!(html.contains("white-space:pre-wrap"), "{html}");
    assert!(html.contains("    eingerueckt"), "{html}");
}

#[test]
fn plain_text_in_multipart_mixed_is_escaped() {
    let raw = "Subject: t\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\n<iframe src=evil>\r\n--b\r\nContent-Type: text/plain; name=a.txt\r\nContent-Disposition: attachment; filename=a.txt\r\n\r\nx\r\n--b--\r\n";
    let d = detail(raw);
    assert!(!d.body_html.contains("<iframe"), "{}", d.body_html);
    assert_eq!(d.attachments.len(), 1);
}

#[test]
fn multiple_inline_body_parts_are_all_shown() {
    // Mailing-list style: text, attachment, footer text.
    let raw = "Subject: t\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nHauptteil\r\n--b\r\nContent-Type: application/octet-stream; name=x.bin\r\nContent-Disposition: attachment; filename=x.bin\r\n\r\nxx\r\n--b\r\nContent-Type: text/plain\r\nContent-Disposition: inline\r\n\r\nListen-Fusszeile\r\n--b--\r\n";
    let d = detail(raw);
    assert!(d.body_html.contains("Hauptteil"), "{}", d.body_html);
    assert!(d.body_html.contains("Listen-Fusszeile"), "{}", d.body_html);
}

// ---- charsets & transfer encodings ----

#[test]
fn charsets() {
    let latin1 = b"Subject: =?iso-8859-1?Q?Gr=FC=DFe?=\r\nContent-Type: text/plain; charset=iso-8859-1\r\n\r\nM\xfcnchen\r\n";
    let s = mime::summary("mb", "INBOX", "x".into(), latin1, None, &[], None);
    assert_eq!(s.subject, "Grüße");
    let d = mime::detail(latin1, s).unwrap();
    assert!(d.body_html.contains("München"), "{}", d.body_html);

    let cp1252 = b"Subject: x\r\nContent-Type: text/plain; charset=windows-1252\r\n\r\n\x80 5 \x93zitat\x94\r\n";
    let d = mime::detail(cp1252, mime::summary("mb", "INBOX", "x".into(), cp1252, None, &[], None)).unwrap();
    assert!(d.body_html.contains("€ 5 “zitat”"), "{}", d.body_html);

    let bom = "Subject: x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n\u{feff}Hallo BOM\r\n";
    assert!(detail(bom).body_html.contains("Hallo BOM"));

    // Unknown charset: shown anyway (no panic, text readable for ASCII).
    let unknown = "Subject: x\r\nContent-Type: text/plain; charset=x-unbekannt-42\r\n\r\nLesbarer Text\r\n";
    assert!(detail(unknown).body_html.contains("Lesbarer Text"));

    // Mislabeled: says us-ascii, contains UTF-8.
    let mislabeled = "Subject: x\r\nContent-Type: text/plain; charset=us-ascii\r\n\r\nGrüße\r\n";
    let d = detail(mislabeled);
    assert!(d.body_html.contains("Gr"), "{}", d.body_html);
}

#[test]
fn quoted_printable_and_base64_variants() {
    let qp = "Subject: x\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nEine sehr lange Zeile, die umbrochen=\r\n wird, Gr=C3=BC=C3=9Fe =3D gleich\r\nKaputt: =ZZ und =\r\n";
    let d = detail(qp);
    assert!(d.body_html.contains("umbrochen wird, Grüße = gleich"), "{}", d.body_html);

    let text = base64::engine::general_purpose::STANDARD.encode("Hallo Base64 Welt, mit Umlauten: äöü");
    let spaced: String = text.chars().collect::<Vec<_>>().chunks(7).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" \r\n\t");
    let b64 = format!("Subject: x\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{spaced}\r\n");
    assert!(detail(&b64).body_html.contains("Umlauten: äöü"), "{}", detail(&b64).body_html);

    // Invalid characters in base64 must not panic.
    let bad = "Subject: x\r\nContent-Type: application/octet-stream; name=a.bin\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=a.bin\r\n\r\n!!!$$$%%%====AAAA\r\n";
    let d = detail(bad);
    assert_eq!(d.attachments.len(), 1);
}

// ---- RFC 2047 / 2231 ----

#[test]
fn encoded_words() {
    let raw = "From: =?UTF-8?B?SsO8cmdlbiBNw7xsbGVy?= <juergen@example.com>\r\n\
To: =?ISO-8859-1?Q?Bj=F6rn?= <bjoern@example.com>, \"Doe, John\" <john@example.com>\r\n\
Subject: =?UTF-8?Q?Gr=C3=BC=C3=9Fe?= =?UTF-8?Q?_aus_K=C3=B6ln?= und =?utf-8?b?bWVocg==?=\r\n\r\nx\r\n";
    let s = summary(raw);
    assert_eq!(s.subject, "Grüße aus Köln und mehr");
    assert_eq!(s.from.as_ref().unwrap().name, "Jürgen Müller");
    assert_eq!(s.to.len(), 2);
    assert_eq!(s.to[0].name, "Björn");
    assert_eq!(s.to[1].name, "Doe, John");
    assert_eq!(s.to[1].address, "john@example.com");

    // Broken encoded words are kept as text, never panic.
    let broken = "Subject: =?UTF-8?B?!!!kaputt?= =?UTF-8?Q?offen\r\nFrom: =?x?q?y?= <a@example.com>\r\n\r\nx\r\n";
    let s = summary(broken);
    assert!(!s.subject.is_empty());
    assert_eq!(s.from.unwrap().address, "a@example.com");
}

#[test]
fn attachment_names() {
    let raw = "Subject: x\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n\
--b\r\nContent-Type: text/plain\r\n\r\nbody\r\n\
--b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename*=UTF-8''Rechnung%20M%C3%A4rz.pdf\r\n\r\nx\r\n\
--b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment;\r\n filename*0*=UTF-8''Sehr%20langer%20;\r\n filename*1*=Name%20%C3%BC.pdf\r\n\r\nx\r\n\
--b\r\nContent-Type: application/pdf; name=\"=?UTF-8?Q?Gr=C3=BC=C3=9Fe.pdf?=\"\r\nContent-Disposition: attachment\r\n\r\nx\r\n\
--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment\r\n\r\nohne Namen\r\n\
--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"../../etc/passwd\"\r\n\r\nx\r\n\
--b--\r\n";
    let d = detail(raw);
    let names: Vec<&str> = d.attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names[0], "Rechnung März.pdf");
    assert_eq!(names[1], "Sehr langer Name ü.pdf");
    assert_eq!(names[2], "Grüße.pdf");
    assert!(names[3].starts_with("anhang"), "{names:?}");
    // Path components never reach the UI (download sanitises again).
    assert!(!names[4].contains('/'), "{names:?}");
}

#[test]
fn attachment_ids_are_stable_and_bounded() {
    let raw = "Subject: x\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nbody\r\n--b\r\nContent-Type: text/plain; name=a.txt\r\nContent-Disposition: attachment; filename=a.txt\r\n\r\nA\r\n--b\r\nContent-Type: text/plain; name=b.txt\r\nContent-Disposition: attachment; filename=b.txt\r\n\r\nB\r\n--b--\r\n";
    let d = detail(raw);
    for a in &d.attachments {
        let (name, _, bytes) = mime::attachment(raw.as_bytes(), a.id.parse().unwrap()).unwrap();
        assert_eq!(name, a.name);
        assert_eq!(bytes.len() as i64, a.size);
    }
    assert!(mime::attachment(raw.as_bytes(), 99).is_err());
}

#[test]
fn message_rfc822_tnef_and_calendar_attachments() {
    let raw = "Subject: Weitergeleitet\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n\
--b\r\nContent-Type: text/plain\r\n\r\nSiehe Anhang\r\n\
--b\r\nContent-Type: message/rfc822\r\n\r\nFrom: inner@example.com\r\nSubject: Innen\r\n\r\nInnerer Text\r\n\
--b\r\nContent-Type: application/ms-tnef; name=\"winmail.dat\"\r\nContent-Disposition: attachment; filename=\"winmail.dat\"\r\nContent-Transfer-Encoding: base64\r\n\r\neJ8+IgAA\r\n\
--b\r\nContent-Type: text/calendar; method=REQUEST; charset=utf-8\r\n\r\nBEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nSUMMARY:Termin\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n\
--b--\r\n";
    let d = detail(raw);
    assert!(d.body_html.contains("Siehe Anhang"), "{}", d.body_html);
    // The calendar text is not shown as body text.
    assert!(!d.body_html.contains("BEGIN:VCALENDAR"), "{}", d.body_html);
    let eml = d.attachments.iter().find(|a| a.content_type == "message/rfc822").expect("eml attachment");
    assert!(eml.name.ends_with(".eml"), "{}", eml.name);
    let (_, _, bytes) = mime::attachment(raw.as_bytes(), eml.id.parse().unwrap()).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("Innerer Text"));
    assert!(d.attachments.iter().any(|a| a.name == "winmail.dat"));
    assert!(d.attachments.iter().any(|a| a.content_type.starts_with("text/calendar")), "{:?}", d.attachments);
}

// ---- inline images ----

#[test]
fn cid_references_with_common_prefix() {
    let one = base64::engine::general_purpose::STANDARD.encode(b"EINS");
    let ten = base64::engine::general_purpose::STANDARD.encode(b"ZEHN");
    let raw = format!(
        "Subject: x\r\nContent-Type: multipart/related; boundary=r\r\n\r\n\
--r\r\nContent-Type: text/html\r\n\r\n<img src=\"cid:img1\"><img src=\"cid:img10\">\r\n\
--r\r\nContent-Type: image/png\r\nContent-ID: <img1>\r\nContent-Transfer-Encoding: base64\r\n\r\n{one}\r\n\
--r\r\nContent-Type: image/png\r\nContent-ID: <img10>\r\nContent-Transfer-Encoding: base64\r\n\r\n{ten}\r\n--r--\r\n"
    );
    let d = detail(&raw);
    assert!(d.body_html.contains(&format!("data:image/png;base64,{one}\"")), "{}", d.body_html);
    assert!(d.body_html.contains(&format!("data:image/png;base64,{ten}\"")), "{}", d.body_html);
    assert!(d.attachments.is_empty(), "{:?}", d.attachments);
}

#[test]
fn hostile_inline_content_type_cannot_break_out_of_attribute() {
    let raw = "Subject: x\r\nContent-Type: multipart/related; boundary=r\r\n\r\n\
--r\r\nContent-Type: text/html\r\n\r\n<img src=\"cid:a\">\r\n\
--r\r\nContent-Type: \"image/png\\\" onerror=\\\"alert(1)\"\r\nContent-ID: <a>\r\n\r\nxx\r\n--r--\r\n";
    let d = detail(raw);
    assert!(!d.body_html.contains(" onerror="), "{}", d.body_html);
    assert!(d.body_html.starts_with("<img src=\"data:image/png"), "{}", d.body_html);
    assert_eq!(d.body_html.matches('"').count(), 2, "{}", d.body_html);
}

#[test]
fn unreferenced_inline_image_stays_an_attachment() {
    let raw = "Subject: x\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>kein Bild</p>\r\n--b\r\nContent-Type: image/jpeg\r\nContent-ID: <foto>\r\nContent-Disposition: inline; filename=foto.jpg\r\n\r\nxx\r\n--b--\r\n";
    let d = detail(raw);
    assert_eq!(d.attachments.len(), 1);
    assert_eq!(d.attachments[0].name, "foto.jpg");
}

// ---- headers ----

#[test]
fn malformed_headers() {
    let raw = "Subject: Erste\r\nSubject: Zweite\r\nDate: kein datum\r\nFrom: \r\nTo: undisclosed-recipients:;\r\nX-Priority: 2 (High)\r\n\r\nx\r\n";
    let s = summary(raw);
    assert!(s.subject == "Erste" || s.subject == "Zweite");
    assert!(s.from.is_none());
    assert!(s.to.is_empty());
    assert_eq!(s.received_at, "");
    assert_eq!(s.importance, "high");

    let folded = "Subject: Ein\r\n  gefalteter\r\n\tBetreff\r\nDate: Tue, 13 Oct 2026 10:00:00 +0200\r\n\r\nx\r\n";
    let s = summary(folded);
    assert_eq!(s.subject.split_whitespace().collect::<Vec<_>>().join(" "), "Ein gefalteter Betreff");
    assert_eq!(s.received_at, "2026-10-13T08:00:00Z");

    // Absurd dates do not panic.
    for date in ["Mon, 32 Foo 99999 99:99:99 +9999", "0", "Thu, 01 Jan 1970 00:00:00 -2400", "Fri, 31 Dec 9999 23:59:59 -1200"] {
        let raw = format!("Date: {date}\r\nSubject: d\r\n\r\nx\r\n");
        let _ = summary(&raw);
    }
}

#[test]
fn address_lists() {
    let raw = "From: \"M\u{fc}ller, Hans\" <hans@example.com>\r\n\
To: Team: a@example.com, \"B, C\" <b@example.com>;, d@example.com\r\n\
Cc: <c@xn--mller-kva.de>, info@m\u{fc}ller.de, \"\" <leer@example.com>, kaputt\r\n\r\nx\r\n";
    let d = detail(raw);
    assert_eq!(d.summary.from.as_ref().unwrap().name, "Müller, Hans");
    let to: Vec<&str> = d.summary.to.iter().map(|a| a.address.as_str()).collect();
    assert!(to.contains(&"a@example.com") && to.contains(&"b@example.com") && to.contains(&"d@example.com"), "{to:?}");
    let cc: Vec<&str> = d.cc.iter().map(|a| a.address.as_str()).collect();
    assert!(cc.contains(&"c@xn--mller-kva.de"), "{cc:?}");
    assert!(cc.contains(&"info@müller.de"), "{cc:?}");
}

#[test]
fn thread_ids_from_headers() {
    let m = |h: &str| {
        let raw = format!("{h}\r\nSubject: x\r\n\r\nx\r\n");
        let parsed = mime::parse(raw.as_bytes()).unwrap();
        mime::thread_id(&parsed)
    };
    assert_eq!(m("Message-ID: <a@x>\r\nReferences: <root@x> <b@x>").as_deref(), Some("root@x"));
    assert_eq!(m("Message-ID: <a@x>\r\nIn-Reply-To: <p@x> (Kommentar)").as_deref(), Some("p@x"));
    assert_eq!(m("Message-ID: <a@x>").as_deref(), Some("a@x"));
    assert_eq!(m("References: kaputt\r\nMessage-ID: <a@x>").as_deref(), Some("a@x"));
    assert_eq!(m("X-Foo: bar"), None);
}

#[test]
fn huge_body_preview_is_bounded() {
    let body = "Wort ".repeat(400_000);
    let raw = format!("Subject: gross\r\nContent-Type: text/plain\r\n\r\n{body}\r\n");
    let s = summary(&raw);
    assert!(s.preview.chars().count() <= 200);
    let d = detail(&raw);
    assert!(d.body_html.len() > 1_000_000);
}

// ---- outgoing ----

#[test]
fn header_injection_via_subject_names_and_attachments() {
    let mut o = outgoing();
    o.subject = "Hallo\r\nBcc: opfer@example.com\r\nX-Evil: 1".into();
    o.to = vec![addr("Bernd\r\nBcc: opfer2@example.com", "bernd@example.com")];
    o.cc = vec![addr("\"Quote\" \\ Name\n", "carl@example.com")];
    o.attachments = vec![(
        "datei.txt\"\r\nContent-Type: text/html\r\nX-Evil2: 1".into(),
        "text/plain\r\nX-Evil3: 1".into(),
        b"inhalt".to_vec(),
    )];
    let raw = mime::build(&o, false).unwrap().formatted();
    let names = header_names(&raw);
    assert!(!names.iter().any(|n| n == "bcc" || n.starts_with("x-evil")), "{names:?}\n{}", String::from_utf8_lossy(&raw));
    let text = String::from_utf8_lossy(&raw);
    for line in text.split("\r\n") {
        assert!(!line.to_ascii_lowercase().starts_with("x-evil"), "{text}");
        assert!(!line.to_ascii_lowercase().starts_with("bcc:"), "{text}");
    }
    // Parsed back the values are intact (minus line breaks).
    let msg = mime::parse(&raw).unwrap();
    assert!(msg.subject().unwrap().contains("Bcc: opfer@example.com"));
    assert_eq!(mime::addresses(msg.to()).len(), 1);
    let parts = mime::all_attachments(&raw);
    assert_eq!(parts.len(), 1);
    assert!(text.contains("filename*0=\"datei.txt' Content-Type: text/html X-Evil2: 1\""), "{text}");
    // Names with colons and quotes survive a draft round trip.
    for name in ["Protokoll: 12.10.txt", "Angebot \"final\".pdf", "a;b=c.txt"] {
        let mut o = outgoing();
        o.attachments = vec![(name.into(), "text/plain".into(), b"x".to_vec())];
        let raw = mime::build(&o, true).unwrap().formatted();
        let back = &mime::all_attachments(&raw)[0].1;
        assert_eq!(back, &name.replace('"', "'"), "{}", String::from_utf8_lossy(&raw));
    }
}

#[test]
fn injection_via_address_is_rejected() {
    for bad in ["bernd@example.com\r\nBcc: x@example.com", "bernd@example.com>, <x@example.com", "kein-at", "", "a@", "@b.de", "a b@c.de"] {
        let mut o = outgoing();
        o.to = vec![addr("", bad)];
        let err = mime::build(&o, false).map(|_| ()).unwrap_err().to_string();
        assert!(err.contains("Ungültige Adresse"), "{bad:?}: {err}");
    }
}

#[test]
fn injection_via_references_is_neutralised() {
    let mut o = outgoing();
    o.in_reply_to = Some("<a@x>\r\nBcc: opfer@example.com".into());
    o.references = Some("<a@x>\r\nX-Evil: 1".into());
    let raw = mime::build(&o, false).unwrap().formatted();
    let names = header_names(&raw);
    assert!(!names.iter().any(|n| n == "bcc" || n == "x-evil"), "{}", String::from_utf8_lossy(&raw));
}

#[test]
fn non_ascii_subject_names_and_filenames_roundtrip() {
    let mut o = outgoing();
    o.subject = "Grüße – 😀 «Test» ".repeat(8).trim().to_string();
    o.from = addr("Jürgen Groß", "anna@example.org");
    o.to = vec![addr("Ünal, Ayşe", "ayse@example.com")];
    o.attachments = vec![("Präsentation März 2026 – final (1).pdf".into(), "application/pdf".into(), vec![1, 2, 3])];
    let raw = mime::build(&o, false).unwrap().formatted();
    // Headers are 7-bit and lines are not overly long.
    let h = headers(&raw);
    assert!(h.is_ascii(), "{h}");
    assert!(h.split("\r\n").all(|l| l.len() <= 998), "{h}");
    let msg = mime::parse(&raw).unwrap();
    assert_eq!(msg.subject(), Some(o.subject.as_str()));
    let from = mime::addresses(msg.from());
    assert_eq!(from[0].name, "Jürgen Groß");
    let to = mime::addresses(msg.to());
    assert_eq!((to[0].name.as_str(), to[0].address.as_str()), ("Ünal, Ayşe", "ayse@example.com"));
    let parts = mime::all_attachments(&raw);
    assert_eq!(parts[0].1, "Präsentation März 2026 – final (1).pdf");
    assert_eq!(parts[0].3, vec![1, 2, 3]);
}

#[test]
fn bcc_only_in_drafts() {
    let mut o = outgoing();
    o.bcc = vec![addr("Geheim", "geheim@example.com")];
    let sent = mime::build(&o, false).unwrap();
    assert!(!header_names(&sent.formatted()).contains(&"bcc".to_string()));
    assert!(sent.envelope().to().iter().any(|a| a.to_string() == "geheim@example.com"));
    let draft = mime::build(&o, true).unwrap().formatted();
    assert!(header_names(&draft).contains(&"bcc".to_string()));
}

#[test]
fn large_attachment_roundtrip() {
    let mut o = outgoing();
    let big: Vec<u8> = (0..8 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    o.attachments = vec![("gross.bin".into(), "application/octet-stream".into(), big.clone())];
    let raw = mime::build(&o, false).unwrap().formatted();
    let parts = mime::all_attachments(&raw);
    assert_eq!(parts[0].3.len(), big.len());
    assert!(parts[0].3 == big);
    // Lines stay within SMTP limits.
    assert!(String::from_utf8_lossy(&raw).split("\r\n").all(|l| l.len() <= 998));
}

#[test]
fn message_id_and_references_headers() {
    let mut o = outgoing();
    o.in_reply_to = Some("<orig@example.com>".into());
    o.references = Some("<root@example.com> <orig@example.com>".into());
    let raw = mime::build(&o, false).unwrap().formatted();
    let msg = mime::parse(&raw).unwrap();
    assert_eq!(msg.header_raw("In-Reply-To").map(str::trim), Some("<orig@example.com>"));
    assert_eq!(mime::thread_id(&msg).as_deref(), Some("root@example.com"));
    assert!(o.message_id.starts_with('<') && o.message_id.ends_with("@example.org>"));
    assert_eq!(mime::message_id_of(&raw).map(|m| format!("<{m}>")), Some(o.message_id.clone()));
    assert!(mime::new_message_id("kaputt").ends_with("@kaputt>") || mime::new_message_id("kaputt").ends_with("@localhost>"));
}

#[test]
fn due_and_category_keywords_are_robust() {
    use mailcore::imap::{category_keyword, is_due_keyword, keyword_category, keyword_due};
    // Malformed due keywords never panic and yield nothing.
    for k in ["$due_", "$due_x", "$due_20261321T0000Z", "$due_20261021T2561Z", "$due_2026-10-21", "$aaaü_x", "$dü", "ü", "$due_20261021T0600Zextra"] {
        let _ = is_due_keyword(k);
        assert_eq!(keyword_due(k), None, "{k}");
    }
    // Malformed category keywords never panic.
    for k in ["$cat_.", "$cat_.zz", "$cat_.d83d", "$cat_.00", "$cat_", "$cat_.0041.", "$CAT_X"] {
        let _ = keyword_category(k);
    }
    for name in ["Mit Leerzeichen", "Ümläüte ß", "(Klammern) [eckig] {geschweift}", "Prozent % Stern * Backslash \\ \"Quote\"", "日本語", " führend"] {
        let k = category_keyword(name);
        assert!(k.bytes().all(|b| b.is_ascii_alphanumeric() || b"$_-.".contains(&b)), "{k}");
        assert_eq!(keyword_category(&k).as_deref(), Some(name));
    }
}

#[test]
fn invitation_shows_time_and_place_from_text_calendar() {
    let raw = "From: Yvonne <yvonne@example.com>\r\nTo: max@example.org\r\nSubject: Symbiosika x WISAG\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHallo zusammen\r\n--b\r\nContent-Type: text/calendar; charset=utf-8; method=REQUEST\r\n\r\nBEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:abc-123\r\nORGANIZER;CN=Yvonne Karolus:mailto:yvonne@example.com\r\nDTSTART;TZID=W. Europe Standard Time:20261102T100000\r\nDTEND;TZID=W. Europe Standard Time:20261102T110000\r\nLOCATION:https://wisag.zoom.us/j/7537235111?pwd=abc\\,\r\n  def\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n--b--\r\n";
    let d = detail(raw);
    let inv = d.invite.expect("invite");
    assert_eq!(inv.message_type, "meetingRequest");
    assert_eq!(inv.start.as_deref(), Some("2026-11-02T09:00:00Z"));
    assert_eq!(inv.end.as_deref(), Some("2026-11-02T10:00:00Z"));
    assert_eq!(inv.location, "https://wisag.zoom.us/j/7537235111?pwd=abc, def");
    assert_eq!(inv.uid, "abc-123");
    assert_eq!(inv.organizer.expect("organizer").name, "Yvonne Karolus");
}

#[test]
fn cancelled_all_day_invitation() {
    let raw = "From: a@example.com\r\nSubject: Abgesagt\r\nContent-Type: text/calendar; method=CANCEL\r\n\r\nBEGIN:VCALENDAR\r\nMETHOD:CANCEL\r\nBEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261105\r\nDTEND;VALUE=DATE:20261106\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let inv = detail(raw).invite.expect("invite");
    assert_eq!(inv.message_type, "meetingCancelled");
    assert!(inv.is_all_day);
    assert_eq!(inv.start.as_deref(), Some("2026-11-05T00:00:00Z"));
}

#[test]
fn plain_mail_has_no_invite() {
    assert!(detail("From: a@example.com\r\nSubject: x\r\n\r\nHallo").invite.is_none());
}
