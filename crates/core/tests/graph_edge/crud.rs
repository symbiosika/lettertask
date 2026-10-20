//! CRUD edge cases: contacts (incl. delta resync), categories, signatures,
//! Kanban lanes – empty, very long and unicode values.


use crate::common::*;
use mailcore::model::*;
use serde_json::{json, Value};

// ------------------------------------------------------------------ contacts

#[tokio::test]
async fn contact_delta_resync_drops_contacts_deleted_meanwhile() {
    let ctx = setup().await;
    let r: SyncResult = call(&ctx.svc, "sync_contacts", json!({"accountId": ctx.account})).await;
    let initial = r.added;
    assert!(initial >= 5);
    // Deleted in Outlook while our delta token expires: the full resync has
    // no "@removed" entries, the stale contact must go anyway.
    let server = ctx.fake.state.contacts(ME);
    let victim = server[0]["id"].as_str().unwrap().to_string();
    assert!(ctx.fake.state.delete_contact(ME, &victim));
    ctx.fake.state.expire_delta_tokens();
    let r: SyncResult = call(&ctx.svc, "sync_contacts", json!({"accountId": ctx.account})).await;
    assert_eq!(r.removed, 1, "{r:?}");
    let all: ContactPage = call(&ctx.svc, "list_contacts", json!({"source": "o365"})).await;
    assert_eq!(all.total, initial - 1);
    assert!(all.items.iter().all(|c| !c.id.ends_with(&victim)));
    // Local and collected contacts survive a full resync.
    let _: Contact = call(&ctx.svc, "create_contact", json!({"accountId": "imap:none", "input": {"displayName": "Lokal"}})).await;
    ctx.fake.state.expire_delta_tokens();
    let r: SyncResult = call(&ctx.svc, "sync_contacts", json!({"accountId": ctx.account})).await;
    assert_eq!((r.added, r.removed), (0, 0), "{r:?}");
    let local: ContactPage = call(&ctx.svc, "list_contacts", json!({"query": "Lokal"})).await;
    assert_eq!(local.total, 1);
    // Incremental again afterwards.
    let r: SyncResult = call(&ctx.svc, "sync_contacts", json!({"accountId": ctx.account})).await;
    assert_eq!((r.added, r.updated, r.removed), (0, 0, 0));
}

#[tokio::test]
async fn contact_values_edge_cases() {
    let ctx = setup().await;
    let create = |input: Value| {
        let svc = ctx.svc.clone();
        let args = json!({"accountId": ctx.account, "input": input});
        async move { svc.dispatch("create_contact", args).await }
    };
    let e = create(json!({})).await.unwrap_err().to_string();
    assert!(e.contains("Namen oder eine E-Mail"), "{e}");
    let e = create(json!({"displayName": "   ", "emails": ["  "]})).await.unwrap_err().to_string();
    assert!(e.contains("Namen oder eine E-Mail"), "{e}");
    let e = create(json!({"displayName": "X", "emails": ["kein-at"]})).await.unwrap_err().to_string();
    assert!(e.contains("Ungültige E-Mail-Adresse"), "{e}");

    // Only an address: it becomes the name.
    let c: Contact = serde_json::from_value(create(json!({"emails": ["  nur@adresse.example "]})).await.unwrap()).unwrap();
    assert_eq!(c.display_name, "nur@adresse.example");
    assert_eq!(c.emails, ["nur@adresse.example"]);

    // Unicode, long notes, blank phones dropped, several numbers.
    let notes = "Notiz 📝 ".repeat(2000);
    let c: Contact = serde_json::from_value(
        create(json!({"givenName": " Zoë ", "surname": "Ñúñez-Øberg", "company": "株式会社テスト", "notes": notes,
            "emails": ["zoe@example.org", "", "zoe.privat@example.org"],
            "phones": [{"kind": "business", "number": "+49 30 1"}, {"kind": "business", "number": "+49 30 2"},
                       {"kind": "mobile", "number": "  "}, {"kind": "home", "number": "+49 89 3"}]}))
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(c.display_name, "Zoë Ñúñez-Øberg");
    assert_eq!(c.company, "株式会社テスト");
    assert_eq!(c.emails.len(), 2);
    assert_eq!(c.phones.len(), 3);
    assert_eq!(c.notes, notes.trim_end());
    let found: ContactPage = call(&ctx.svc, "list_contacts", json!({"query": "ñúñez"})).await;
    assert_eq!(found.total, 1, "search is case-insensitive for non-ASCII too");

    // Search terms with SQL wildcard/escape characters are literal.
    for q in ["%", "_", "\\", "'", "\"", "100%", "a_b", "\\%"] {
        let r = ctx.svc.dispatch("list_contacts", json!({"query": q})).await;
        assert!(r.is_ok(), "{q:?}: {r:?}");
    }
    let pct: ContactPage = call(&ctx.svc, "list_contacts", json!({"query": "%"})).await;
    assert_eq!(pct.total, 0, "'%' must not match everything");

    // Update + delete twice.
    let u: Contact = call(&ctx.svc, "update_contact", json!({"id": c.id, "input": {"displayName": "Zoë N.", "emails": ["zoe@example.org"]}})).await;
    assert_eq!(u.display_name, "Zoë N.");
    assert_eq!(u.emails, ["zoe@example.org"]);
    let _: Value = call(&ctx.svc, "delete_contact", json!({"id": c.id})).await;
    let e = call_err(&ctx.svc, "delete_contact", json!({"id": c.id})).await;
    assert!(e.contains("Kontakt"), "{e}");
    let e = call_err(&ctx.svc, "update_contact", json!({"id": "gibt|es-nicht", "input": {"displayName": "x"}})).await;
    assert!(e.contains("Kontakt"), "{e}");
}

#[tokio::test]
async fn contact_deleted_on_server_meanwhile_can_still_be_removed() {
    let ctx = setup().await;
    let _: SyncResult = call(&ctx.svc, "sync_contacts", json!({"accountId": ctx.account})).await;
    let all: ContactPage = call(&ctx.svc, "list_contacts", json!({"source": "o365"})).await;
    let c = all.items[0].clone();
    let gid = c.id.split_once('|').unwrap().1;
    assert!(ctx.fake.state.delete_contact(ME, gid));
    let _: Value = call(&ctx.svc, "delete_contact", json!({"id": c.id})).await;
    assert!(ctx.svc.dispatch("get_contact", json!({"id": c.id})).await.is_err());
}

// ---------------------------------------------------------------- categories

#[tokio::test]
async fn category_edge_cases() {
    let ctx = setup().await;
    let create = |name: &str, color: &str| {
        let svc = ctx.svc.clone();
        let args = json!({"accountId": ctx.account, "displayName": name, "color": color});
        async move { svc.dispatch("create_category", args).await }
    };
    let c: Category = serde_json::from_value(create("  Kundenprojekt 🚀 ", "preset3").await.unwrap()).unwrap();
    assert_eq!(c.display_name, "Kundenprojekt 🚀");
    let e = create("KUNDENPROJEKT 🚀", "preset1").await.unwrap_err().to_string();
    assert!(e.contains("existiert bereits"), "{e}");
    let e = create("   ", "preset1").await.unwrap_err().to_string();
    assert!(e.contains("fehlt"), "{e}");
    let e = create(&"x".repeat(256), "none").await.unwrap_err().to_string();
    assert!(e.contains("zu lang"), "{e}");
    assert!(create(&"y".repeat(255), "none").await.is_ok());
    for bad in ["preset25", "preset+1", "red", "", "preset"] {
        let e = create("Farbe", bad).await.unwrap_err().to_string();
        assert!(e.contains("Ungültige Farbe"), "{bad}: {e}");
    }
    let before = ctx.fake.state.request_count();
    let _ = create("Farbe", "lila").await;
    assert_eq!(ctx.fake.state.request_count(), before, "validated locally");

    let up: Category = call(&ctx.svc, "update_category", json!({"accountId": ctx.account, "id": c.id, "color": "none"})).await;
    assert_eq!(up.color, "none");
    let e = call_err(&ctx.svc, "update_category", json!({"accountId": ctx.account, "id": "weg", "color": "none"})).await;
    assert!(e.contains("404"), "{e}");
    // Deleting is idempotent (e.g. removed in Outlook meanwhile).
    let _: Value = call(&ctx.svc, "delete_category", json!({"accountId": ctx.account, "id": c.id})).await;
    let _: Value = call(&ctx.svc, "delete_category", json!({"accountId": ctx.account, "id": c.id})).await;
    let cats: Vec<Category> = call(&ctx.svc, "list_categories", json!({"accountId": ctx.account})).await;
    assert!(!cats.iter().any(|x| x.id == c.id));
}

// ---------------------------------------------------------------- signatures

#[tokio::test]
async fn signature_edge_cases() {
    let ctx = setup().await;
    let s: SignatureSettings = call(&ctx.svc, "get_signatures", json!({"accountId": ctx.account})).await;
    assert!(s.signatures.is_empty() && s.default_id.is_none());
    let html = format!("<p>Mit freundlichen Grüßen<br>Zoë 😀</p>{}", "<p>x</p>".repeat(5000));
    let saved: SignatureSettings = call(
        &ctx.svc,
        "save_signatures",
        json!({"accountId": ctx.account, "settings": {
            "signatures": [
                {"id": "", "name": "", "html": html},
                {"id": "dup", "name": "Kurz", "html": "<p>MfG</p>"},
                {"id": "dup", "name": "Kopie", "html": "<p>Kopie</p>"}
            ],
            "defaultId": "gibt-es-nicht", "useOnNew": true, "useOnReply": false, "useOnForward": true}}),
    )
    .await;
    let ids: std::collections::HashSet<&str> = saved.signatures.iter().map(|x| x.id.as_str()).collect();
    assert_eq!(ids.len(), 3, "ids are unique");
    assert!(saved.signatures.iter().all(|x| !x.id.is_empty()));
    assert_eq!(saved.signatures[0].name, "Signatur 1");
    assert_eq!(saved.default_id.as_deref(), Some(saved.signatures[0].id.as_str()), "unknown default → first");
    assert_eq!(saved.signatures[0].html, html);
    let again: SignatureSettings = call(&ctx.svc, "get_signatures", json!({"accountId": ctx.account})).await;
    assert_eq!(again, saved);

    // Applied per mode.
    let new: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "new"})).await;
    assert!(new.body_html.contains("Mit freundlichen Grüßen"));
    let inbox = ctx.folder("inbox").await;
    let m = ctx.list(&inbox.id).await.items[0].clone();
    let reply: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "reply", "sourceMessageId": m.id})).await;
    assert!(!reply.body_html.contains("Mit freundlichen Grüßen"), "not on replies");
    let fw: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "forward", "sourceMessageId": m.id})).await;
    assert!(fw.body_html.contains("Mit freundlichen Grüßen"));

    // Empty list: no default, no signature.
    let empty: SignatureSettings = call(
        &ctx.svc,
        "save_signatures",
        json!({"accountId": ctx.account, "settings": {"signatures": [], "defaultId": "x"}}),
    )
    .await;
    assert!(empty.default_id.is_none());
    let e = call_err(&ctx.svc, "save_signatures", json!({"accountId": "gibt-es-nicht", "settings": {"signatures": []}})).await;
    assert!(e.contains("Konto"), "{e}");
}

// --------------------------------------------------------------------- lanes

#[tokio::test]
async fn lane_edge_cases() {
    let ctx = setup().await;
    let save = |lanes: Value| {
        let svc = ctx.svc.clone();
        let args = json!({"accountId": ctx.account, "lanes": lanes});
        async move { svc.dispatch("save_lanes", args).await }
    };
    let ok: Vec<String> = serde_json::from_value(save(json!(["Überprüfung", "Wartet_auf-Antwort", "a".repeat(30)])).await.unwrap()).unwrap();
    assert_eq!(ok.len(), 3);
    let e = save(json!(["a".repeat(31)])).await.unwrap_err().to_string();
    assert!(e.contains("Ungültiger Lane-Name"), "{e}");
    let e = save(json!(["Erledigt ✅"])).await.unwrap_err().to_string();
    assert!(e.contains("Ungültiger Lane-Name"), "{e}");
    let e = save(json!(["Neu", "NEU"])).await.unwrap_err().to_string();
    assert!(e.contains("doppelt"), "{e}");
    let e = save(json!(["Über", "über"])).await.unwrap_err().to_string();
    assert!(e.contains("doppelt"), "non-ASCII case too: {e}");

    // Saving the same lanes again creates nothing twice.
    let cats_before: Vec<Category> = call(&ctx.svc, "list_categories", json!({"accountId": ctx.account})).await;
    let _ = save(json!(ok)).await.unwrap();
    let cats_after: Vec<Category> = call(&ctx.svc, "list_categories", json!({"accountId": ctx.account})).await;
    assert_eq!(cats_before.len(), cats_after.len());

    // Mail in a lane.
    let inbox = ctx.folder("inbox").await;
    let m = ctx.list(&inbox.id).await.items[0].clone();
    let _: Value = call(&ctx.svc, "set_lane", json!({"mailboxId": ctx.account, "ids": [m.id], "lane": "Überprüfung"})).await;

    // Changing only the case would delete the shared Outlook category and
    // strip the lane from every mail: rejected, nothing changes.
    let e = save(json!(["überprüfung", "Wartet_auf-Antwort"])).await.unwrap_err().to_string();
    assert!(e.contains("Groß-/Kleinschreibung"), "{e}");
    let still = ctx.list(&inbox.id).await.items.into_iter().find(|x| x.id == m.id).unwrap();
    assert!(still.categories.contains(&"[K_Überprüfung]".to_string()));
    let cats: Vec<Category> = call(&ctx.svc, "list_categories", json!({"accountId": ctx.account})).await;
    assert!(cats.iter().any(|c| c.display_name == "[K_Überprüfung]"));

    // The lane category was already created by another client: fine.
    let _: Category = call(&ctx.svc, "create_category", json!({"accountId": ctx.account, "displayName": "[K_Extern]", "color": "none"})).await;
    let lanes: Vec<String> = serde_json::from_value(save(json!(["Überprüfung", "Extern"])).await.unwrap()).unwrap();
    assert_eq!(lanes, ["Überprüfung", "Extern"]);

    // All lanes removed: categories gone, mails cleaned.
    let lanes: Vec<String> = serde_json::from_value(save(json!([])).await.unwrap()).unwrap();
    assert!(lanes.is_empty());
    let cats: Vec<Category> = call(&ctx.svc, "list_categories", json!({"accountId": ctx.account})).await;
    assert!(!cats.iter().any(|c| c.display_name.starts_with("[K_")));
    let m2 = ctx.list(&inbox.id).await.items.into_iter().find(|x| x.id == m.id).unwrap();
    assert!(!m2.categories.iter().any(|c| c.starts_with("[K_")));
    let e = call_err(&ctx.svc, "set_lane", json!({"mailboxId": ctx.account, "ids": [m.id], "lane": "Überprüfung"})).await;
    assert!(e.contains("existiert nicht"), "{e}");
}

/// Graph sometimes answers a delta round with only the changed properties of a
/// message. The cached row must keep sender, subject and date (otherwise the
/// list shows "(Unbekannt)" and the mail jumps to the end).
#[tokio::test]
async fn sparse_delta_items_keep_sender_subject_and_date() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    ctx.sync(&inbox.id).await;
    ctx.fake.state.set_sparse_delta(true);

    let before = ctx.list(&inbox.id).await.items;
    let target = before.iter().find(|m| !m.is_read).expect("unread demo mail").clone();
    let _: Value = call(&ctx.svc, "set_read", json!({"mailboxId": ctx.account, "ids": [target.id], "isRead": true})).await;
    ctx.sync(&inbox.id).await;
    let after = ctx.list(&inbox.id).await.items;
    let m = after.iter().find(|m| m.id == target.id).unwrap();
    assert!(m.is_read);
    assert_eq!(m.subject, target.subject);
    assert_eq!(m.from, target.from);
    assert_eq!(m.received_at, target.received_at);
    assert_eq!(after.iter().map(|m| &m.id).collect::<Vec<_>>(), before.iter().map(|m| &m.id).collect::<Vec<_>>(), "order unchanged");

    // A new mail announced only partially is fetched in full.
    let id = ctx.fake.state.inject_incoming(ME, "inbox", json!({"subject": "Teilweise gemeldet"}));
    ctx.sync(&inbox.id).await;
    let page = ctx.list(&inbox.id).await;
    let m = page.items.iter().find(|m| m.id == id).expect("new mail cached");
    assert_eq!(m.subject, "Teilweise gemeldet");
    assert!(!m.received_at.is_empty());
    assert!(m.from.is_some());
    assert_eq!(page.items[0].id, id, "newest first");
}

#[tokio::test]
async fn account_picture_is_validated_and_listed() {
    let ctx = setup().await;
    let png = "data:image/png;base64,iVBORw0KGgo=";
    let _: Value = call(&ctx.svc, "set_account_avatar", json!({"accountId": ctx.account, "avatar": png})).await;
    let accounts: Vec<Account> = call(&ctx.svc, "list_accounts", json!({})).await;
    assert_eq!(accounts[0].avatar.as_deref(), Some(png));

    for bad in [
        "javascript:alert(1)",
        "data:image/svg+xml;base64,PHN2Zz4=",
        "data:text/html;base64,PGI+",
        "data:image/png;base64,***",
    ] {
        call_err(&ctx.svc, "set_account_avatar", json!({"accountId": ctx.account, "avatar": bad})).await;
    }
    let big = format!("data:image/png;base64,{}", "A".repeat(400_000));
    assert!(call_err(&ctx.svc, "set_account_avatar", json!({"accountId": ctx.account, "avatar": big})).await.contains("zu groß"));
    call_err(&ctx.svc, "set_account_avatar", json!({"accountId": "unbekannt", "avatar": null})).await;

    let _: Value = call(&ctx.svc, "set_account_avatar", json!({"accountId": ctx.account, "avatar": null})).await;
    let accounts: Vec<Account> = call(&ctx.svc, "list_accounts", json!({})).await;
    assert_eq!(accounts[0].avatar, None);
    let _: Value = call(&ctx.svc, "set_account_order", json!({"accountIds": [ctx.account]})).await;
}

#[tokio::test]
async fn account_name_and_badge_are_validated_and_listed() {
    let ctx = setup().await;
    let look = json!({"accountId": ctx.account, "label": "  Büro  ", "badge": {"color": "#D0567A", "text": " CUCO \n\n LHV "}});
    let _: Value = call(&ctx.svc, "set_account_look", look).await;
    let accounts: Vec<Account> = call(&ctx.svc, "list_accounts", json!({})).await;
    assert_eq!(accounts[0].label.as_deref(), Some("Büro"));
    let badge = accounts[0].badge.clone().expect("badge");
    assert_eq!(badge.color.as_deref(), Some("#d0567a"));
    assert_eq!(badge.text.as_deref(), Some("CUCO\nLHV"));

    for bad in [
        json!({"label": "x".repeat(61)}),
        json!({"badge": {"color": "red"}}),
        json!({"badge": {"color": "#12345g"}}),
        json!({"badge": {"text": "NEUNZEICH"}}),
        json!({"badge": {"text": "A\nB\nC"}}),
    ] {
        let mut args = bad;
        args["accountId"] = json!(ctx.account);
        call_err(&ctx.svc, "set_account_look", args).await;
    }
    call_err(&ctx.svc, "set_account_look", json!({"accountId": "unbekannt"})).await;

    // empty = defaults again (address, initials, derived colour)
    let _: Value = call(&ctx.svc, "set_account_look", json!({"accountId": ctx.account, "label": " ", "badge": {"text": ""}})).await;
    let accounts: Vec<Account> = call(&ctx.svc, "list_accounts", json!({})).await;
    assert_eq!(accounts[0].label, None);
    assert_eq!(accounts[0].badge, None);
}
