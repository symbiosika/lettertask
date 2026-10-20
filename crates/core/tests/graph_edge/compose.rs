//! Graph compose edge cases: reply/replyAll/forward, attachments, pasted
//! images, failing sends and reminders.


use base64::Engine;
use crate::common::*;
use mailcore::model::*;
use serde_json::{json, Value};

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn addr(name: &str, a: &str) -> Value {
    json!({"emailAddress": {"name": name, "address": a}})
}

fn draft(ctx: &Ctx, d: &Value) -> Value {
    let mut base = json!({"id": null, "mailboxId": ctx.account, "mode": "new", "to": [], "cc": [], "bcc": [],
        "subject": "", "bodyHtml": "", "importance": "normal", "requestReadReceipt": false,
        "newAttachments": [], "removedAttachmentIds": [], "remindAt": null});
    for (k, v) in d.as_object().unwrap() {
        base[k] = v.clone();
    }
    base
}

fn drafts_on_server(ctx: &Ctx, subject: &str) -> Vec<Value> {
    ctx.fake.state.messages_in(ME, "drafts").into_iter().filter(|m| m["subject"] == json!(subject)).collect()
}

#[tokio::test]
async fn reply_reply_all_and_forward() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    ctx.sync(&inbox.id).await;
    let id = ctx.fake.state.inject_incoming(
        ME,
        "inbox",
        json!({"subject": "Projektstand", "from": addr("Erika", "erika@fabrikam.example"),
            "toRecipients": [addr("Max", ME), addr("Bob", "bob@fabrikam.example")],
            "ccRecipients": [addr("Carol", "carol@fabrikam.example"), addr("Max", ME.to_uppercase().as_str())]}),
    );
    ctx.sync(&inbox.id).await;

    let r: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "reply", "sourceMessageId": id})).await;
    assert_eq!(r.mode, "reply");
    assert_eq!(r.to.iter().map(|a| a.address.as_str()).collect::<Vec<_>>(), ["erika@fabrikam.example"]);
    assert!(r.cc.is_empty());
    assert!(r.subject.starts_with("AW: "));

    let ra: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "replyAll", "sourceMessageId": id})).await;
    let to: Vec<&str> = ra.to.iter().map(|a| a.address.as_str()).collect();
    assert!(to.contains(&"erika@fabrikam.example") && to.contains(&"bob@fabrikam.example"), "{to:?}");
    assert!(!to.iter().any(|a| a.eq_ignore_ascii_case(ME)), "never reply to myself");
    assert_eq!(ra.cc.iter().map(|a| a.address.as_str()).collect::<Vec<_>>(), ["carol@fabrikam.example"]);

    // Forward of a mail with attachments carries them.
    let page = ctx.list(&inbox.id).await;
    let with_att = page.items.iter().find(|m| m.has_attachments).expect("seed mail with attachment");
    let fw: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "forward", "sourceMessageId": with_att.id})).await;
    assert!(fw.to.is_empty());
    assert!(fw.subject.starts_with("WG: "));
    assert!(!fw.attachments.is_empty());

    let e = call_err(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "reply"})).await;
    assert!(e.contains("Ursprungsnachricht fehlt"), "{e}");
    let e = call_err(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "weiterleiten", "sourceMessageId": id})).await;
    assert!(e.contains("Unbekannter Modus"), "{e}");
    let e = call_err(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "reply", "sourceMessageId": "AAMkAD-weg"})).await;
    assert!(e.contains("404"), "{e}");
}

#[tokio::test]
async fn attachments_add_remove_and_large() {
    let ctx = setup().await;
    let small = base64::engine::general_purpose::STANDARD.encode(b"Hallo Welt");
    let big_bytes: Vec<u8> = (0..(3 * 1024 * 1024 + 4321)).map(|i| (i % 251) as u8).collect();
    let big = base64::engine::general_purpose::STANDARD.encode(&big_bytes);
    let d: Draft = call(
        &ctx.svc,
        "save_draft",
        json!({"draft": draft(&ctx, &json!({"subject": "Anhänge", "to": [{"name": "", "address": "erika@fabrikam.example"}],
            "newAttachments": [
                {"name": "notiz.txt", "contentType": "text/plain", "contentBase64": small},
                {"name": "groß – äöü.bin", "contentType": "", "contentBase64": big}
            ]}))}),
    )
    .await;
    let id = d.id.clone().unwrap();
    assert_eq!(d.attachments.len(), 2);
    let on_server = ctx.fake.state.attachments(ME, &id);
    let big_att = on_server.iter().find(|(n, _, _)| n == "groß – äöü.bin").expect("large attachment");
    assert_eq!(big_att.2, big_bytes, "chunked upload is byte-exact");
    assert_eq!(big_att.1, "application/octet-stream", "empty content type gets a default");

    // Remove one, plus an id that is already gone (no error).
    let txt = d.attachments.iter().find(|a| a.name == "notiz.txt").unwrap().id.clone();
    let d2: Draft = call(
        &ctx.svc,
        "save_draft",
        json!({"draft": draft(&ctx, &json!({"id": id, "subject": "Anhänge", "to": [{"name": "", "address": "erika@fabrikam.example"}],
            "removedAttachmentIds": [txt, "AAMkAD-gibt-es-nicht"]}))}),
    )
    .await;
    assert_eq!(d2.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["groß – äöü.bin"]);

    // Broken input is rejected in German.
    let e = call_err(
        &ctx.svc,
        "save_draft",
        json!({"draft": draft(&ctx, &json!({"id": id, "newAttachments": [{"name": "x.bin", "contentBase64": "%%%"}]}))}),
    )
    .await;
    assert!(e.contains("Anhang x.bin"), "{e}");
    let e = call_err(
        &ctx.svc,
        "save_draft",
        json!({"draft": draft(&ctx, &json!({"id": id, "newAttachments": [{"name": " ", "contentBase64": small}]}))}),
    )
    .await;
    assert!(e.contains("Dateinamen"), "{e}");
    for bad in ["erika", "erika@", "@fabrikam.example", "Erika <erika@fabrikam.example>", "a b@c.de", "x@y.example,z@y.example"] {
        let e = call_err(&ctx.svc, "save_draft", json!({"draft": draft(&ctx, &json!({"to": [{"name": "", "address": bad}]}))})).await;
        assert!(e.contains("Ungültige Adresse"), "{bad}: {e}");
    }
}

#[tokio::test]
async fn pasted_images_become_inline_attachments() {
    let ctx = setup().await;
    let body = format!(
        "<p>Screenshot:</p><img src=\"data:image/png;base64,{PNG}\"><p>nochmal</p><img alt='x' src='data:image/png;base64,{PNG}'><img src=\"data:image/png;base64,kaputt!\">"
    );
    let d: Draft = call(
        &ctx.svc,
        "save_draft",
        json!({"draft": draft(&ctx, &json!({"subject": "Bild", "to": [{"name": "", "address": "erika@fabrikam.example"}], "bodyHtml": body}))}),
    )
    .await;
    let id = d.id.clone().unwrap();
    let server = ctx.fake.state.message(ME, &id).unwrap();
    let html = server["body"]["content"].as_str().unwrap();
    assert_eq!(html.matches("cid:").count(), 2, "{html}");
    assert!(!html.contains(PNG), "image bytes are not in the HTML any more");
    assert!(html.contains("data:image/png;base64,kaputt!"), "invalid data is left alone");
    let atts = ctx.fake.state.attachments(ME, &id);
    assert_eq!(atts.len(), 1, "identical images share one attachment");
    assert_eq!(atts[0].1, "image/png");
    // In the editor the image is visible again and not offered as attachment.
    assert!(d.body_html.contains("#cid="), "{}", d.body_html);
    assert!(d.attachments.is_empty());
    // Saving the loaded draft again does not duplicate anything.
    let d2: Draft = call(
        &ctx.svc,
        "save_draft",
        json!({"draft": draft(&ctx, &json!({"id": id, "subject": "Bild", "to": [{"name": "", "address": "erika@fabrikam.example"}], "bodyHtml": d.body_html}))}),
    )
    .await;
    assert_eq!(ctx.fake.state.attachments(ME, &id).len(), 1);
    assert_eq!(d2.body_html, d.body_html);
    // Sent mail carries the inline image.
    let _: Value = call(
        &ctx.svc,
        "send_draft",
        json!({"draft": draft(&ctx, &json!({"id": id, "subject": "Bild", "to": [{"name": "", "address": "erika@fabrikam.example"}], "bodyHtml": d2.body_html}))}),
    )
    .await;
    let sent = ctx.fake.state.sent_messages(ME).into_iter().find(|m| m["subject"] == "Bild").unwrap();
    assert!(sent["body"]["content"].as_str().unwrap().contains("cid:"));
}

#[tokio::test]
async fn failed_send_keeps_the_draft() {
    let ctx = setup().await;
    let input = draft(
        &ctx,
        &json!({"subject": "Wichtig", "to": [{"name": "Erika", "address": "erika@fabrikam.example"}], "bodyHtml": "<p>Text</p>",
            "newAttachments": [{"name": "a.txt", "contentType": "text/plain", "contentBase64": base64::engine::general_purpose::STANDARD.encode(b"A")}]}),
    );
    ctx.fake.state.inject_fault(graph_fake::Fault::status(500).on_path("/send"));
    let e = call_err(&ctx.svc, "send_draft", json!({"draft": input})).await;
    assert!(e.contains("Entwürfe"), "{e}");
    let kept = drafts_on_server(&ctx, "Wichtig");
    assert_eq!(kept.len(), 1, "draft with attachment stays on the server");
    let draft_id = kept[0]["id"].as_str().unwrap().to_string();
    assert_eq!(ctx.fake.state.attachments(ME, &draft_id).len(), 1);
    assert!(ctx.fake.state.sent_messages(ME).iter().all(|m| m["subject"] != "Wichtig"));
    // It shows up in the local Drafts folder.
    let drafts = ctx.folder("drafts").await;
    assert!(ctx.list(&drafts.id).await.items.iter().any(|m| m.id == draft_id));

    // Retrying with the draft id sends exactly once and removes the draft.
    let mut retry = draft(&ctx, &json!({"id": draft_id, "subject": "Wichtig", "to": [{"name": "Erika", "address": "erika@fabrikam.example"}], "bodyHtml": "<p>Text</p>"}));
    retry["mode"] = json!("new");
    let _: Value = call(&ctx.svc, "send_draft", json!({"draft": retry})).await;
    assert!(drafts_on_server(&ctx, "Wichtig").is_empty());
    let sent: Vec<Value> = ctx.fake.state.sent_messages(ME).into_iter().filter(|m| m["subject"] == "Wichtig").collect();
    assert_eq!(sent.len(), 1);
}

#[tokio::test]
async fn invalid_reminder_is_rejected_before_sending() {
    let ctx = setup().await;
    let past = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let before = ctx.fake.state.sent_messages(ME).len();
    for remind in [past.as_str(), "nächsten Montag"] {
        let input = draft(&ctx, &json!({"subject": "Mit Erinnerung", "to": [{"name": "", "address": "erika@fabrikam.example"}], "remindAt": remind}));
        let e = call_err(&ctx.svc, "send_draft", json!({"draft": input})).await;
        assert!(e.contains("Fälligkeitszeitpunkt") || e.contains("Vergangenheit"), "{e}");
    }
    assert_eq!(ctx.fake.state.sent_messages(ME).len(), before, "nothing was sent");
    assert!(drafts_on_server(&ctx, "Mit Erinnerung").is_empty(), "and no draft was created");
    // Blank reminder = none.
    let input = draft(&ctx, &json!({"subject": "Ohne Erinnerung", "to": [{"name": "", "address": "erika@fabrikam.example"}], "remindAt": "  "}));
    let _: Value = call(&ctx.svc, "send_draft", json!({"draft": input})).await;
    assert_eq!(ctx.fake.state.sent_messages(ME).len(), before + 1);
}

#[tokio::test]
async fn reminder_failure_after_send_is_not_a_send_failure() {
    let ctx = setup().await;
    let remind = (chrono::Utc::now() + chrono::Duration::days(2)).to_rfc3339();
    // The mail goes out, creating the reminder mail fails on the server.
    let snoozed = ctx.svc.dispatch("create_task", json!({"mailboxId": ctx.account, "subject": "init", "due": remind})).await;
    assert!(snoozed.is_ok(), "creates the Wiedervorlage folder");
    let folder = ctx.folder("snoozed").await;
    ctx.fake.state.inject_fault(graph_fake::Fault::status(500).on_path(&format!("{}/messages", folder.id)));
    let input = draft(&ctx, &json!({"subject": "Gesendet trotz Fehler", "to": [{"name": "", "address": "erika@fabrikam.example"}], "remindAt": remind}));
    let _: Value = call(&ctx.svc, "send_draft", json!({"draft": input})).await;
    assert_eq!(ctx.fake.state.sent_messages(ME).iter().filter(|m| m["subject"] == "Gesendet trotz Fehler").count(), 1);
    let failed = ctx.emitted("reminder-failed");
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["subject"], "Gesendet trotz Fehler");
}

#[tokio::test]
async fn recipients_and_discard_edge_cases() {
    let ctx = setup().await;
    let e = call_err(
        &ctx.svc,
        "send_draft",
        json!({"draft": draft(&ctx, &json!({"subject": "Leer", "to": [{"name": "", "address": "  "}]}))}),
    )
    .await;
    assert!(e.contains("mindestens einen Empfänger"), "{e}");
    // Blank entries next to a real one are dropped; Bcc only is enough.
    let _: Value = call(
        &ctx.svc,
        "send_draft",
        json!({"draft": draft(&ctx, &json!({"subject": "Nur Bcc", "bcc": [{"name": "", "address": "geheim@fabrikam.example"}]}))}),
    )
    .await;
    // Discarding nothing or an already deleted draft is fine.
    let _: Value = call(&ctx.svc, "discard_draft", json!({"mailboxId": ctx.account, "draftId": null})).await;
    let _: Value = call(&ctx.svc, "discard_draft", json!({"mailboxId": ctx.account, "draftId": "AAMkAD-schon-weg"})).await;
    // Suggestions: short queries return nothing, quotes are harmless.
    let s: Vec<Address> = call(&ctx.svc, "suggest_recipients", json!({"mailboxId": ctx.account, "query": "e"})).await;
    assert!(s.is_empty());
    let s: Vec<Address> = call(&ctx.svc, "suggest_recipients", json!({"mailboxId": ctx.account, "query": "\"Erika\" OR 1=1"})).await;
    assert!(s.len() <= 10);
}
