use graph_fake::{spawn, FakeOptions, FakeServer};
use reqwest::{Client, Method};
use serde_json::{json, Value};

struct T {
    srv: FakeServer,
    c: Client,
    tok: String,
}

async fn setup(seed_demo: bool) -> T {
    let srv = spawn(FakeOptions { seed_demo, now: chrono::Utc::now() }).await;
    let tok = srv.issue_token();
    let c = Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    T { srv, c, tok }
}

impl T {
    fn url(&self, path: &str) -> String {
        if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}/v1.0{}", self.srv.base_url, path)
        }
    }
    async fn req(&self, m: Method, path: &str, body: Option<Value>, headers: &[(&str, &str)]) -> (u16, Value) {
        let mut r = self.c.request(m, self.url(path)).bearer_auth(&self.tok);
        for (k, v) in headers {
            r = r.header(*k, *v);
        }
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r.send().await.unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }
    async fn get(&self, path: &str) -> (u16, Value) {
        self.req(Method::GET, path, None, &[]).await
    }
    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.req(Method::POST, path, Some(body), &[]).await
    }
    async fn patch(&self, path: &str, body: Value) -> (u16, Value) {
        self.req(Method::PATCH, path, Some(body), &[]).await
    }
    async fn delete(&self, path: &str) -> u16 {
        self.req(Method::DELETE, path, None, &[]).await.0
    }
    /// GETs all pages following @odata.nextLink.
    async fn get_all(&self, path: &str, headers: &[(&str, &str)]) -> Vec<Value> {
        let mut out = Vec::new();
        let mut next = Some(path.to_string());
        while let Some(p) = next {
            let (s, v) = self.req(Method::GET, &p, None, headers).await;
            assert_eq!(s, 200, "{v}");
            out.extend(v["value"].as_array().unwrap().iter().cloned());
            next = v["@odata.nextLink"].as_str().map(str::to_string);
        }
        out
    }
}

fn b64url(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

#[tokio::test]
async fn auth_code_pkce_and_refresh() {
    let t = setup(false).await;
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk-and-some-more-entropy";
    use sha2::Digest;
    let challenge = b64url(&sha2::Sha256::digest(verifier.as_bytes()));
    let redirect = "http://localhost:53682/callback";
    let authorize = |ch: &str| {
        format!(
            "{}/organizations/oauth2/v2.0/authorize?client_id=app1&response_type=code&redirect_uri={}&scope={}&state=xyz&code_challenge={}&code_challenge_method=S256",
            t.srv.base_url,
            urlenc(redirect),
            urlenc("offline_access https://graph.microsoft.com/Mail.ReadWrite User.Read"),
            ch
        )
    };
    let get_code = |loc: &str| -> (String, String) {
        let u = url::Url::parse(loc).unwrap();
        let q: std::collections::HashMap<String, String> = u.query_pairs().into_owned().collect();
        (q["code"].clone(), q["state"].clone())
    };

    let r = t.c.get(authorize(&challenge)).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 302);
    let loc = r.headers()["location"].to_str().unwrap().to_string();
    assert!(loc.starts_with(redirect));
    let (code, state) = get_code(&loc);
    assert_eq!(state, "xyz");

    let token_url = format!("{}/organizations/oauth2/v2.0/token", t.srv.base_url);
    // wrong verifier
    let r = t.c.post(&token_url).form(&[("grant_type", "authorization_code"), ("client_id", "app1"), ("code", &code), ("redirect_uri", redirect), ("code_verifier", "wrong")]).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 400);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"], "invalid_grant");

    // code is single-use: get a new one
    let r = t.c.get(authorize(&challenge)).send().await.unwrap();
    let (code, _) = get_code(r.headers()["location"].to_str().unwrap());
    let r = t.c.post(&token_url).form(&[("grant_type", "authorization_code"), ("client_id", "app1"), ("code", &code), ("redirect_uri", redirect), ("code_verifier", verifier)]).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["token_type"], "Bearer");
    assert_eq!(v["scope"], "Mail.ReadWrite User.Read");
    let at = v["access_token"].as_str().unwrap();
    let rt = v["refresh_token"].as_str().unwrap();

    let me: Value = t.c.get(t.url("/me")).bearer_auth(at).send().await.unwrap().json().await.unwrap();
    assert_eq!(me["mail"], "max.mustermann@contoso.example");
    assert_eq!(me["displayName"], "Max Mustermann");

    // refresh
    let r = t.c.post(&token_url).form(&[("grant_type", "refresh_token"), ("client_id", "app1"), ("refresh_token", rt), ("scope", "offline_access Mail.ReadWrite")]).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let v: Value = r.json().await.unwrap();
    assert_ne!(v["refresh_token"].as_str().unwrap(), rt);
    let r = t.c.post(&token_url).form(&[("grant_type", "refresh_token"), ("client_id", "app1"), ("refresh_token", "bogus")]).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 400);

    // missing / expired token
    let r = t.c.get(t.url("/me")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "InvalidAuthenticationToken");
    t.srv.state.expire_tokens();
    assert_eq!(t.get("/me").await.0, 401);
}

fn urlenc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[tokio::test]
async fn me_select_shared_and_unknown_users() {
    let t = setup(true).await;
    let (s, me) = t.get("/me?$select=id,displayName,mail,userPrincipalName").await;
    assert_eq!(s, 200);
    assert!(me.get("givenName").is_none());
    assert_eq!(me["userPrincipalName"], "max.mustermann@contoso.example");
    let (s, v) = t.get("/users/datenschutz@contoso.example/mailFolders/inbox/messages?$top=50").await;
    assert_eq!(s, 200);
    assert_eq!(v["value"].as_array().unwrap().len(), 6);
    let (s, v) = t.get("/users/datenschutz%40contoso.example/mailFolders/inbox").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["displayName"], "Posteingang");
    let (s, v) = t.get("/users/nobody@contoso.example/mailFolders").await;
    assert_eq!(s, 404);
    assert_eq!(v["error"]["code"], "ErrorInvalidUser");
    let (s, v) = t.get("/me/bogus").await;
    assert_eq!(s, 400);
    assert!(v["error"]["message"].is_string());
}

#[tokio::test]
async fn folders_crud() {
    let t = setup(true).await;
    let (_, root) = t.get("/me/mailFolders/msgfolderroot?$select=id").await;
    let root_id = root["id"].as_str().unwrap();
    let (s, v) = t.get("/me/mailFolders?$top=100").await;
    assert_eq!(s, 200);
    let names: Vec<&str> = v["value"].as_array().unwrap().iter().map(|f| f["displayName"].as_str().unwrap()).collect();
    for n in ["Posteingang", "Entwürfe", "Gesendete Elemente", "Gelöschte Elemente", "Archiv", "Junk-E-Mail", "Postausgang"] {
        assert!(names.contains(&n), "{n} missing in {names:?}");
    }
    assert!(v["value"].as_array().unwrap().iter().all(|f| f["parentFolderId"] == root_id));
    // default page size 10 → paging works too
    let (_, inbox) = t.get("/me/mailFolders/inbox").await;
    assert!(inbox["unreadItemCount"].as_u64().unwrap() > 0);
    assert_eq!(inbox["childFolderCount"], 2);
    let inbox_id = inbox["id"].as_str().unwrap().to_string();
    let (_, kids) = t.get(&format!("/me/mailFolders/{inbox_id}/childFolders")).await;
    let kid_names: Vec<&str> = kids["value"].as_array().unwrap().iter().map(|f| f["displayName"].as_str().unwrap()).collect();
    assert_eq!(kid_names, vec!["Projekte", "Rechnungen"]);

    let (s, f) = t.post(&format!("/me/mailFolders/{inbox_id}/childFolders"), json!({ "displayName": "Neu" })).await;
    assert_eq!(s, 201);
    assert_eq!(f["parentFolderId"], inbox_id.as_str());
    let (s, _) = t.post(&format!("/me/mailFolders/{inbox_id}/childFolders"), json!({ "displayName": "neu" })).await;
    assert_eq!(s, 409);
    let fid = f["id"].as_str().unwrap();
    let (s, f2) = t.patch(&format!("/me/mailFolders/{fid}"), json!({ "displayName": "Umbenannt" })).await;
    assert_eq!(s, 200);
    assert_eq!(f2["displayName"], "Umbenannt");
    assert_eq!(t.delete(&format!("/me/mailFolders/{fid}")).await, 204);
    let (_, moved) = t.get(&format!("/me/mailFolders/{fid}")).await;
    let deleted_id = t.srv.state.folder_id("me", "deleteditems").unwrap();
    assert_eq!(moved["parentFolderId"], deleted_id.as_str());
    assert_eq!(t.delete(&format!("/me/mailFolders/{fid}")).await, 204);
    assert_eq!(t.get(&format!("/me/mailFolders/{fid}")).await.0, 404);
    assert_eq!(t.delete("/me/mailFolders/inbox").await, 400);
    let (s, _) = t.post("/me/mailFolders", json!({ "displayName": "Top" })).await;
    assert_eq!(s, 201);
}

#[tokio::test]
async fn messages_paging_filter_search() {
    let t = setup(true).await;
    let all = t.get_all("/me/mailFolders/inbox/messages?$top=7&$orderby=receivedDateTime%20desc", &[]).await;
    assert!(all.len() >= 40, "only {}", all.len());
    let dates: Vec<&str> = all.iter().map(|m| m["receivedDateTime"].as_str().unwrap()).collect();
    let mut sorted = dates.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(dates, sorted);
    let mid = dates[10];
    let older = t.get_all(&format!("/me/mailFolders/inbox/messages?$select=id,subject,receivedDateTime&$orderby=receivedDateTime%20desc&$top=100&$filter=receivedDateTime%20lt%20{mid}"), &[]).await;
    assert!(older.iter().all(|m| m["receivedDateTime"].as_str().unwrap() < mid));
    assert!(older[0].get("body").is_none(), "select must drop body");
    let unread = t.get_all("/me/mailFolders/inbox/messages?$filter=isRead%20eq%20false&$select=id,isRead", &[]).await;
    assert!(!unread.is_empty() && unread.iter().all(|m| m["isRead"] == false));
    let (s, v) = t.get("/me/mailFolders/inbox/messages?$filter=contains(subject,'x')").await;
    assert_eq!(s, 400);
    assert_eq!(v["error"]["code"], "ErrorInvalidUrlQueryFilter");

    let (s, v) = t.get("/me/messages?$search=%22Rechnung%22&$top=50").await;
    assert_eq!(s, 200);
    let subjects: Vec<&str> = v["value"].as_array().unwrap().iter().map(|m| m["subject"].as_str().unwrap()).collect();
    assert!(subjects.iter().any(|s| s.contains("Rechnung 2026-1043")), "{subjects:?}");
    let (_, v) = t.get("/me/mailFolders/inbox/messages?$search=%22thomas%20staging%22").await;
    assert_eq!(v["value"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn delta_initial_and_incremental() {
    let t = setup(false).await;
    let st = &t.srv.state;
    let old = st.inject_incoming("me", "inbox", json!({ "subject": "alt", "receivedDateTime": "2020-01-01T00:00:00Z" }));
    let ids: Vec<String> = (0..3).map(|i| st.inject_incoming("me", "inbox", json!({ "subject": format!("m{i}") }))).collect();
    let prefer = [("Prefer", "odata.maxpagesize=2")];

    let mut next = "/me/mailFolders/inbox/messages/delta?$select=subject,isRead,receivedDateTime&$filter=receivedDateTime%20ge%202026-01-01T00:00:00Z".to_string();
    let mut seen = Vec::new();
    let delta_link;
    let mut pages = 0;
    loop {
        let (s, v) = t.req(Method::GET, &next, None, &prefer).await;
        assert_eq!(s, 200, "{v}");
        pages += 1;
        for m in v["value"].as_array().unwrap() {
            assert!(m.get("body").is_none());
            seen.push(m["id"].as_str().unwrap().to_string());
        }
        if let Some(n) = v["@odata.nextLink"].as_str() {
            next = n.to_string();
        } else {
            delta_link = v["@odata.deltaLink"].as_str().unwrap().to_string();
            break;
        }
    }
    assert_eq!(pages, 2);
    seen.sort();
    let mut expected = ids.clone();
    expected.sort();
    assert_eq!(seen, expected, "old message filtered out");
    assert!(!seen.contains(&old));

    // nothing changed
    let (_, v) = t.get(&delta_link).await;
    assert_eq!(v["value"].as_array().unwrap().len(), 0);
    let delta_link = v["@odata.deltaLink"].as_str().unwrap().to_string();

    // changes
    let added = st.inject_incoming("me", "inbox", json!({ "subject": "neu" }));
    let (s, _) = t.patch(&format!("/me/messages/{}", ids[0]), json!({ "isRead": true })).await;
    assert_eq!(s, 200);
    let (s, moved) = t.post(&format!("/me/messages/{}/move", ids[1]), json!({ "destinationId": "archive" })).await;
    assert_eq!(s, 201);
    assert_ne!(moved["id"], ids[1].as_str());
    assert_eq!(t.get(&format!("/me/messages/{}", ids[1])).await.0, 404);
    assert_eq!(t.delete(&format!("/me/messages/{}", ids[2])).await, 204);

    let (_, v) = t.get(&delta_link).await;
    let vals = v["value"].as_array().unwrap();
    let find = |id: &str| vals.iter().find(|m| m["id"] == id).cloned();
    assert_eq!(find(&added).unwrap()["subject"], "neu");
    assert_eq!(find(&ids[0]).unwrap()["isRead"], true);
    assert!(find(&ids[0]).unwrap().get("body").is_none(), "select remembered in delta link");
    assert_eq!(find(&ids[1]).unwrap()["@removed"]["reason"], "deleted");
    assert_eq!(find(&ids[2]).unwrap()["@removed"]["reason"], "deleted");
    assert_eq!(vals.len(), 4);

    // moved message shows up in archive delta
    let archive = t.get_all("/me/mailFolders/archive/messages/delta", &[]).await;
    assert_eq!(archive.len(), 1);

    let (s, v) = t.get("/me/mailFolders/inbox/messages/delta?$deltatoken=garbage").await;
    assert_eq!(s, 400);
    assert_eq!(v["error"]["code"], "SyncStateInvalid");
}

#[tokio::test]
async fn patch_rules_and_event_expand() {
    let t = setup(true).await;
    let inbox = t.srv.state.messages_in("me", "inbox");
    let m = &inbox[0];
    let id = m["id"].as_str().unwrap();
    let (s, v) = t.patch(&format!("/me/messages/{id}"), json!({ "flag": { "flagStatus": "flagged" }, "categories": ["Wichtig"], "isRead": false })).await;
    assert_eq!(s, 200);
    assert_eq!(v["flag"]["flagStatus"], "flagged");
    assert_eq!(v["categories"], json!(["Wichtig"]));
    let (s, v) = t.patch(&format!("/me/messages/{id}"), json!({ "body": { "contentType": "text", "content": "x" } })).await;
    assert_eq!(s, 400, "body is draft-only: {v}");

    let invite = inbox.iter().find(|m| m["meetingMessageType"] == "meetingRequest").expect("invitation seeded");
    assert_eq!(invite["@odata.type"], "#microsoft.graph.eventMessageRequest");
    let iid = invite["id"].as_str().unwrap();
    let (s, v) = t.get(&format!("/me/messages/{iid}?$select=id&$expand=microsoft.graph.eventMessage%2Fevent(%24select%3Did)")).await;
    assert_eq!(s, 200);
    let eid = v["event"]["id"].as_str().expect("expanded event");
    let (s, ev) = t.req(Method::GET, &format!("/me/events/{eid}"), None, &[("Prefer", "outlook.timezone=\"UTC\"")]).await;
    assert_eq!(s, 200);
    assert_eq!(ev["isOrganizer"], false);
    assert_eq!(ev["responseStatus"]["response"], "notResponded");
    let (_, cal) = t.get(&format!("/me/events/{eid}/calendar")).await;
    assert_eq!(cal["name"], "Kalender");

    // respond
    let (s, _) = t.post(&format!("/me/events/{eid}/accept"), json!({ "comment": "Passt", "sendResponse": true })).await;
    assert_eq!(s, 202);
    let (_, ev) = t.get(&format!("/me/events/{eid}")).await;
    assert_eq!(ev["responseStatus"]["response"], "accepted");
    let (s, _) = t.post(&format!("/me/events/{eid}/tentativelyAccept"), json!({ "comment": "", "sendResponse": false })).await;
    assert_eq!(s, 202);
    let (_, ev) = t.get(&format!("/me/events/{eid}")).await;
    assert_eq!(ev["responseStatus"]["response"], "tentativelyAccepted");
    assert_eq!(ev["showAs"], "tentative");
    let (s, _) = t.post(&format!("/me/events/{eid}/decline"), json!({ "comment": "", "sendResponse": true })).await;
    assert_eq!(s, 202);
    assert_eq!(t.get(&format!("/me/events/{eid}")).await.0, 404);

    // inline + file attachments in seed
    let inline = inbox.iter().find(|m| m["body"]["content"].as_str().unwrap().contains("cid:")).unwrap();
    assert_eq!(inline["hasAttachments"], false);
    let (_, atts) = t.get(&format!("/me/messages/{}/attachments?$select=id,name,contentType,size,isInline", inline["id"].as_str().unwrap())).await;
    let a = &atts["value"][0];
    assert_eq!(a["isInline"], true);
    assert!(a.get("contentBytes").is_none());
    let (_, full) = t.get(&format!("/me/messages/{}/attachments/{}", inline["id"].as_str().unwrap(), a["id"].as_str().unwrap())).await;
    assert_eq!(full["contentId"], "banner001@contoso.example");
    assert!(full["contentBytes"].as_str().unwrap().starts_with("iVBORw0KGgo"));
    let pdf = inbox.iter().find(|m| m["subject"] == "Rechnung 2026-1043").unwrap();
    assert_eq!(pdf["hasAttachments"], true);
}

#[tokio::test]
async fn reply_forward_and_send() {
    let t = setup(false).await;
    let st = &t.srv.state;
    let id = st.inject_incoming(
        "me",
        "inbox",
        json!({
            "subject": "Projekt",
            "from": { "emailAddress": { "name": "Erika", "address": "erika@fabrikam.example" } },
            "toRecipients": [
                { "emailAddress": { "name": "Max Mustermann", "address": "max.mustermann@contoso.example" } },
                { "emailAddress": { "name": "Jonas", "address": "jonas@contoso.example" } }
            ],
            "ccRecipients": [{ "emailAddress": { "name": "Laura", "address": "laura@contoso.example" } }],
            "body": { "contentType": "html", "content": "<html><body><p>Original-Text</p></body></html>" }
        }),
    );
    let (s, _) = t.post(&format!("/me/messages/{id}/attachments"), json!({ "@odata.type": "#microsoft.graph.fileAttachment", "name": "a.txt", "contentType": "text/plain", "contentBytes": "SGFsbG8=" })).await;
    assert_eq!(s, 201);

    let (s, r) = t.post(&format!("/me/messages/{id}/createReply"), json!({})).await;
    assert_eq!(s, 201);
    assert_eq!(r["subject"], "AW: Projekt");
    assert_eq!(r["isDraft"], true);
    assert_eq!(r["toRecipients"].as_array().unwrap().len(), 1);
    assert_eq!(r["toRecipients"][0]["emailAddress"]["address"], "erika@fabrikam.example");
    assert_eq!(r["hasAttachments"], false);
    let body = r["body"]["content"].as_str().unwrap();
    assert!(body.contains("<b>Von:</b>") && body.contains("Original-Text") && body.contains("<b>Betreff:</b> Projekt"));

    let (_, ra) = t.post(&format!("/me/messages/{id}/createReplyAll"), json!({})).await;
    let to: Vec<&str> = ra["toRecipients"].as_array().unwrap().iter().map(|r| r["emailAddress"]["address"].as_str().unwrap()).collect();
    assert_eq!(to, vec!["erika@fabrikam.example", "jonas@contoso.example"]);
    assert_eq!(ra["ccRecipients"][0]["emailAddress"]["address"], "laura@contoso.example");

    let (_, fw) = t.post(&format!("/me/messages/{id}/createForward"), json!({})).await;
    assert_eq!(fw["subject"], "WG: Projekt");
    assert_eq!(fw["hasAttachments"], true);
    let fid = fw["id"].as_str().unwrap();
    // sending a draft without recipients fails
    let (s, v) = t.post(&format!("/me/messages/{fid}/send"), json!({})).await;
    assert_eq!(s, 400);
    assert_eq!(v["error"]["code"], "ErrorInvalidRecipients");
    let (s, _) = t.patch(&format!("/me/messages/{fid}"), json!({ "toRecipients": [{ "emailAddress": { "address": "datenschutz@contoso.example", "name": "DS" } }] })).await;
    assert_eq!(s, 200);
    let (s, _) = t.post(&format!("/me/messages/{fid}/send"), json!({})).await;
    assert_eq!(s, 202);
    assert_eq!(t.get(&format!("/me/messages/{fid}")).await.0, 404);
    let sent = st.sent_messages("me");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["isDraft"], false);
    assert_eq!(sent[0]["subject"], "WG: Projekt");
    let shared_inbox = st.messages_in("datenschutz@contoso.example", "inbox");
    assert_eq!(shared_inbox.len(), 1);
    assert_eq!(shared_inbox[0]["isRead"], false);
    assert_eq!(shared_inbox[0]["hasAttachments"], true);
    assert_eq!(st.attachments("datenschutz@contoso.example", shared_inbox[0]["id"].as_str().unwrap())[0].2, b"Hallo");

    // sendMail with inline attachments, to myself
    let (s, _) = t
        .post(
            "/me/sendMail",
            json!({ "message": {
                "subject": "Direkt",
                "body": { "contentType": "html", "content": "<p>Hi</p>" },
                "toRecipients": [{ "emailAddress": { "address": "max.mustermann@contoso.example" } }],
                "attachments": [{ "@odata.type": "#microsoft.graph.fileAttachment", "name": "x.bin", "contentBytes": "AAEC" }]
            }, "saveToSentItems": true }),
        )
        .await;
    assert_eq!(s, 202);
    assert_eq!(st.sent_messages("me").len(), 2);
    assert!(st.messages_in("me", "inbox").iter().any(|m| m["subject"] == "Direkt" && m["isRead"] == false));
    let (s, _) = t.post("/me/sendMail", json!({ "message": { "subject": "x", "toRecipients": [] } })).await;
    assert_eq!(s, 400);

    // drafts: create, patch, send
    let (s, d) = t.post("/me/messages", json!({ "subject": "Entwurf", "body": { "contentType": "html", "content": "<p>x</p>" } })).await;
    assert_eq!(s, 201);
    assert_eq!(d["isDraft"], true);
    assert_eq!(d["parentFolderId"], st.folder_id("me", "drafts").unwrap().as_str());
    let did = d["id"].as_str().unwrap();
    let (s, _) = t.patch(&format!("/me/messages/{did}"), json!({ "toRecipients": [{ "emailAddress": { "address": "someone@example.com" } }], "body": { "contentType": "html", "content": "<p>neu</p>" } })).await;
    assert_eq!(s, 200);
    let (_, d2) = t.get(&format!("/me/messages/{did}")).await;
    assert_eq!(d2["bodyPreview"], "neu");
    assert_eq!(t.post(&format!("/me/messages/{did}/send"), json!({})).await.0, 202);
    assert!(!st.messages_in("me", "drafts").iter().any(|m| m["id"] == did));
    assert_eq!(st.messages_in("me", "drafts").len(), 2, "reply + replyAll drafts remain");

    // copy keeps original
    let (s, c) = t.post(&format!("/me/messages/{id}/copy"), json!({ "destinationId": "archive" })).await;
    assert_eq!(s, 201);
    assert_ne!(c["id"], id.as_str());
    assert_eq!(t.get(&format!("/me/messages/{id}")).await.0, 200);

    // MIME
    let r = t.c.get(t.url(&format!("/me/messages/{id}/$value"))).bearer_auth(&t.tok).send().await.unwrap();
    let mime = r.text().await.unwrap();
    assert!(mime.contains("Subject: Projekt") && mime.contains("multipart/mixed"));
}

#[tokio::test]
async fn upload_session_large_attachment() {
    let t = setup(false).await;
    let (_, d) = t.post("/me/messages", json!({ "subject": "Groß" })).await;
    let did = d["id"].as_str().unwrap();
    let big = vec![7u8; 4 * 1024 * 1024 + 100];
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&big);
    let (s, v) = t.post(&format!("/me/messages/{did}/attachments"), json!({ "@odata.type": "#microsoft.graph.fileAttachment", "name": "big.bin", "contentBytes": b64 })).await;
    assert_eq!(s, 413, "{v}");

    let (s, sess) = t.post(&format!("/me/messages/{did}/attachments/createUploadSession"), json!({ "AttachmentItem": { "attachmentType": "file", "name": "big.bin", "size": big.len(), "contentType": "application/octet-stream" } })).await;
    assert_eq!(s, 201);
    let url = sess["uploadUrl"].as_str().unwrap();
    assert_eq!(sess["nextExpectedRanges"][0], "0-");
    let chunk = 3 * 1024 * 1024;
    let r = t.c.put(url).header("Content-Range", format!("bytes 0-{}/{}", chunk - 1, big.len())).body(big[..chunk].to_vec()).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["nextExpectedRanges"][0], format!("{chunk}-"));
    // wrong offset
    let r = t.c.put(url).header("Content-Range", format!("bytes 0-9/{}", big.len())).body(vec![0u8; 10]).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 416);
    let r = t.c.put(url).header("Content-Range", format!("bytes {}-{}/{}", chunk, big.len() - 1, big.len())).body(big[chunk..].to_vec()).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 201);

    let (_, atts) = t.get(&format!("/me/messages/{did}/attachments")).await;
    assert_eq!(atts["value"][0]["size"], big.len());
    let aid = atts["value"][0]["id"].as_str().unwrap();
    let r = t.c.get(t.url(&format!("/me/messages/{did}/attachments/{aid}/$value"))).bearer_auth(&t.tok).send().await.unwrap();
    assert_eq!(r.headers()["content-type"], "application/octet-stream");
    assert_eq!(r.bytes().await.unwrap().len(), big.len());
    assert_eq!(t.delete(&format!("/me/messages/{did}/attachments/{aid}")).await, 204);
    let (_, atts) = t.get(&format!("/me/messages/{did}/attachments")).await;
    assert!(atts["value"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn master_categories() {
    let t = setup(true).await;
    let (_, v) = t.get("/me/outlook/masterCategories").await;
    let names: Vec<&str> = v["value"].as_array().unwrap().iter().map(|c| c["displayName"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Wichtig", "Business & Pitch", "Info", "Privat", "Rechnung", "Warten auf Antwort"]);
    let (s, c) = t.post("/me/outlook/masterCategories", json!({ "displayName": "Kunde", "color": "preset9" })).await;
    assert_eq!(s, 201);
    assert_eq!(t.post("/me/outlook/masterCategories", json!({ "displayName": "kunde", "color": "preset1" })).await.0, 409);
    assert_eq!(t.post("/me/outlook/masterCategories", json!({ "displayName": "X", "color": "pink" })).await.0, 400);
    let cid = c["id"].as_str().unwrap();
    let (s, c2) = t.patch(&format!("/me/outlook/masterCategories/{cid}"), json!({ "color": "preset2" })).await;
    assert_eq!(s, 200);
    assert_eq!(c2["color"], "preset2");
    assert_eq!(t.patch(&format!("/me/outlook/masterCategories/{cid}"), json!({ "displayName": "Anders" })).await.0, 400);
    assert_eq!(t.delete(&format!("/me/outlook/masterCategories/{cid}")).await, 204);
    assert_eq!(t.srv.state.categories("me").len(), 6);
    let (_, v) = t.get("/users/datenschutz@contoso.example/outlook/masterCategories").await;
    assert_eq!(v["value"].as_array().unwrap().len(), 2);
}

const UTC: (&str, &str) = ("Prefer", "outlook.timezone=\"UTC\"");

#[tokio::test]
async fn calendar_view_recurrence_and_timezones() {
    let t = setup(false).await;
    let (_, cals) = t.get("/me/calendars?$top=100&$select=id,name,color,hexColor,canEdit,isDefaultCalendar").await;
    let cal = &cals["value"][0];
    assert_eq!(cal["isDefaultCalendar"], true);
    let cid = cal["id"].as_str().unwrap();

    // Weekly Monday 09:30 Europe/Berlin across the DST change (25.10.2026)
    let (s, ev) = t
        .req(
            Method::POST,
            &format!("/me/calendars/{cid}/events"),
            Some(json!({
                "subject": "Jour fixe",
                "start": { "dateTime": "2026-10-12T09:30:00", "timeZone": "Europe/Berlin" },
                "end": { "dateTime": "2026-10-12T10:30:00", "timeZone": "Europe/Berlin" },
                "recurrence": { "pattern": { "type": "weekly", "interval": 1, "daysOfWeek": ["monday"] },
                                "range": { "type": "noEnd", "startDate": "2026-10-12" } },
                "attendees": [{ "emailAddress": { "address": "a@x.example", "name": "A" }, "type": "required" }],
                "isOnlineMeeting": true
            })),
            &[UTC],
        )
        .await;
    assert_eq!(s, 201, "{ev}");
    assert_eq!(ev["type"], "seriesMaster");
    // Like Graph: POST answers in the event's own zone, not the Prefer zone.
    assert_eq!(ev["start"]["dateTime"], "2026-10-12T09:30:00.0000000");
    assert_eq!(ev["start"]["timeZone"], "Europe/Berlin");
    let (_, got) = t.req(Method::GET, &format!("/me/events/{}", ev["id"].as_str().unwrap()), None, &[UTC]).await;
    assert_eq!(got["start"]["dateTime"], "2026-10-12T07:30:00.0000000");
    assert_eq!(got["start"]["timeZone"], "UTC");
    assert_eq!(ev["recurrence"]["range"]["recurrenceTimeZone"], "Europe/Berlin");
    assert_eq!(ev["recurrence"]["range"]["endDate"], "0001-01-01");
    assert_eq!(ev["attendees"][0]["status"]["response"], "none");
    assert!(ev["onlineMeeting"]["joinUrl"].as_str().unwrap().starts_with("https://teams.microsoft.com/"));
    let master = ev["id"].as_str().unwrap().to_string();

    // numbered daily series: 3 occurrences
    let (s, _) = t
        .req(
            Method::POST,
            "/me/events",
            Some(json!({
                "subject": "Daily x3",
                "start": { "dateTime": "2026-10-20T08:00:00", "timeZone": "UTC" },
                "end": { "dateTime": "2026-10-20T08:15:00", "timeZone": "UTC" },
                "recurrence": { "pattern": { "type": "daily", "interval": 1 }, "range": { "type": "numbered", "startDate": "2026-10-20", "numberOfOccurrences": 3 } }
            })),
            &[UTC],
        )
        .await;
    assert_eq!(s, 201);
    // all-day single
    let (s, _) = t
        .post(
            "/me/events",
            json!({ "subject": "Ganztägig", "isAllDay": true,
                    "start": { "dateTime": "2026-10-21T00:00:00", "timeZone": "Europe/Berlin" },
                    "end": { "dateTime": "2026-10-22T00:00:00", "timeZone": "Europe/Berlin" } }),
        )
        .await;
    assert_eq!(s, 201);
    let (s, v) = t.post("/me/events", json!({ "subject": "bad", "isAllDay": true, "start": { "dateTime": "2026-10-21T10:00:00", "timeZone": "UTC" }, "end": { "dateTime": "2026-10-22T00:00:00", "timeZone": "UTC" } })).await;
    assert_eq!(s, 400, "{v}");
    let (s, _) = t.post("/me/events", json!({ "subject": "bad", "start": { "dateTime": "2026-10-21T10:00:00", "timeZone": "Mars/Base" }, "end": { "dateTime": "2026-10-21T11:00:00", "timeZone": "UTC" } })).await;
    assert_eq!(s, 400);

    let view = format!("/me/calendars/{cid}/calendarView?startDateTime=2026-10-12T00:00:00Z&endDateTime=2026-11-03T00:00:00Z");
    let items = t.get_all(&view, &[UTC]).await;
    let jf: Vec<&Value> = items.iter().filter(|e| e["subject"] == "Jour fixe").collect();
    let starts: Vec<&str> = jf.iter().map(|e| e["start"]["dateTime"].as_str().unwrap()).collect();
    assert_eq!(starts, vec!["2026-10-12T07:30:00.0000000", "2026-10-19T07:30:00.0000000", "2026-10-26T08:30:00.0000000", "2026-11-02T08:30:00.0000000"]);
    assert!(jf.iter().all(|e| e["type"] == "occurrence" && e["seriesMasterId"] == master.as_str()));
    assert_eq!(items.iter().filter(|e| e["subject"] == "Daily x3").count(), 3);
    let allday = items.iter().find(|e| e["subject"] == "Ganztägig").unwrap();
    assert_eq!(allday["isAllDay"], true);
    assert_eq!(allday["start"]["dateTime"], "2026-10-21T00:00:00.0000000");
    // sorted by start
    let all_starts: Vec<&str> = items.iter().map(|e| e["start"]["dateTime"].as_str().unwrap()).collect();
    let mut sorted = all_starts.clone();
    sorted.sort();
    assert_eq!(all_starts, sorted);

    // Berlin view shows local wall time
    let berlin = t.get_all(&view, &[("Prefer", "outlook.timezone=\"Europe/Berlin\"")]).await;
    let b: Vec<&str> = berlin.iter().filter(|e| e["subject"] == "Jour fixe").map(|e| e["start"]["dateTime"].as_str().unwrap()).collect();
    assert!(b.iter().all(|s| s.ends_with("T09:30:00.0000000")), "{b:?}");

    // missing window
    let (s, v) = t.get(&format!("/me/calendars/{cid}/calendarView")).await;
    assert_eq!(s, 400);
    assert_eq!(v["error"]["code"], "ErrorInvalidParameter");

    // edit one occurrence → exception; delete another
    let occ19 = jf[1]["id"].as_str().unwrap().to_string();
    let occ26 = jf[2]["id"].as_str().unwrap().to_string();
    let (s, ex) = t
        .req(Method::PATCH, &format!("/me/events/{occ19}"), Some(json!({ "subject": "Jour fixe (verschoben)", "start": { "dateTime": "2026-10-19T11:00:00", "timeZone": "UTC" }, "end": { "dateTime": "2026-10-19T12:00:00", "timeZone": "UTC" } })), &[UTC])
        .await;
    assert_eq!(s, 200, "{ex}");
    assert_eq!(ex["type"], "exception");
    // PATCH answers in the series' zone (CEST): 11:00 UTC = 13:00.
    assert_eq!(ex["start"]["dateTime"], "2026-10-19T13:00:00.0000000");
    assert_eq!(ex["start"]["timeZone"], "Europe/Berlin");
    assert_eq!(t.delete(&format!("/me/events/{occ26}")).await, 204);
    let items = t.get_all(&view, &[UTC]).await;
    let jf: Vec<(&str, &str)> = items
        .iter()
        .filter(|e| e["seriesMasterId"] == master.as_str())
        .map(|e| (e["subject"].as_str().unwrap(), e["start"]["dateTime"].as_str().unwrap()))
        .collect();
    assert_eq!(jf, vec![("Jour fixe", "2026-10-12T07:30:00.0000000"), ("Jour fixe (verschoben)", "2026-10-19T11:00:00.0000000"), ("Jour fixe", "2026-11-02T08:30:00.0000000")]);
    let (s, single) = t.get(&format!("/me/events/{occ19}")).await;
    assert_eq!(s, 200);
    assert_eq!(single["type"], "exception");

    // instances
    let inst = t.get_all(&format!("/me/events/{master}/instances?startDateTime=2026-10-01T00:00:00Z&endDateTime=2026-10-20T00:00:00Z"), &[UTC]).await;
    assert_eq!(inst.len(), 2);

    // organizer can't respond
    let (s, _) = t.post(&format!("/me/events/{master}/accept"), json!({ "sendResponse": true })).await;
    assert_eq!(s, 400);

    // update series master, then delete
    let (s, upd) = t.req(Method::PATCH, &format!("/me/events/{master}"), Some(json!({ "subject": "JF neu", "categories": ["Info"], "location": { "displayName": "Raum 1" } })), &[UTC]).await;
    assert_eq!(s, 200);
    assert_eq!(upd["location"]["displayName"], "Raum 1");
    assert_eq!(upd["start"]["dateTime"], "2026-10-12T09:30:00.0000000");
    assert_eq!(t.delete(&format!("/me/events/{master}")).await, 204);
    assert_eq!(t.get(&format!("/me/events/{master}")).await.0, 404);
    assert_eq!(t.srv.state.events("me").len(), 2);
}

#[tokio::test]
async fn monthly_and_yearly_patterns() {
    let t = setup(false).await;
    let create = |subject: &str, start: &str, rec: Value| {
        json!({ "subject": subject, "start": { "dateTime": start, "timeZone": "UTC" }, "end": { "dateTime": start.replace("T10", "T11"), "timeZone": "UTC" }, "recurrence": rec })
    };
    assert_eq!(t.post("/me/events", create("M", "2026-01-31T10:00:00", json!({ "pattern": { "type": "absoluteMonthly", "interval": 1, "dayOfMonth": 31 }, "range": { "type": "endDate", "startDate": "2026-01-31", "endDate": "2026-12-31" } }))).await.0, 201);
    assert_eq!(t.post("/me/events", create("R", "2026-01-05T10:00:00", json!({ "pattern": { "type": "relativeMonthly", "interval": 1, "daysOfWeek": ["monday"], "index": "first" }, "range": { "type": "numbered", "startDate": "2026-01-01", "numberOfOccurrences": 4 } }))).await.0, 201);
    assert_eq!(t.post("/me/events", create("Y", "2025-03-15T10:00:00", json!({ "pattern": { "type": "absoluteYearly", "interval": 1, "dayOfMonth": 15, "month": 3 }, "range": { "type": "noEnd", "startDate": "2025-03-15" } }))).await.0, 201);
    let (s, v) = t.post("/me/events", create("bad", "2026-01-05T10:00:00", json!({ "pattern": { "type": "weekly", "interval": 1 }, "range": { "type": "noEnd", "startDate": "2026-01-05" } }))).await;
    assert_eq!(s, 400, "{v}");
    let (s, _) = t.post("/me/events", create("bad", "2026-01-05T10:00:00", json!({ "pattern": { "type": "daily", "interval": 1 }, "range": { "type": "noEnd" } }))).await;
    assert_eq!(s, 400);

    let items = t.get_all("/me/calendarView?startDateTime=2026-01-01T00:00:00Z&endDateTime=2027-01-01T00:00:00Z&$top=50", &[UTC]).await;
    let dates = |s: &str| -> Vec<String> { items.iter().filter(|e| e["subject"] == s).map(|e| e["start"]["dateTime"].as_str().unwrap()[..10].to_string()).collect() };
    // Day 31 falls on the last day of shorter months (as in Outlook).
    assert_eq!(
        dates("M"),
        vec![
            "2026-01-31", "2026-02-28", "2026-03-31", "2026-04-30", "2026-05-31", "2026-06-30", "2026-07-31", "2026-08-31",
            "2026-09-30", "2026-10-31", "2026-11-30", "2026-12-31"
        ]
    );
    assert_eq!(dates("R"), vec!["2026-01-05", "2026-02-02", "2026-03-02", "2026-04-06"]);
    assert_eq!(dates("Y"), vec!["2026-03-15"]);
}

#[tokio::test]
async fn demo_calendars_and_readonly() {
    let t = setup(true).await;
    let (_, cals) = t.get("/me/calendars").await;
    let cals = cals["value"].as_array().unwrap().clone();
    assert_eq!(cals.len(), 3);
    let hol = cals.iter().find(|c| c["name"] == "Feiertage").unwrap();
    assert_eq!(hol["canEdit"], false);
    assert_eq!(hol["hexColor"], "#2e7d32");
    let hid = hol["id"].as_str().unwrap();
    let (s, v) = t.post(&format!("/me/calendars/{hid}/events"), json!({ "subject": "x", "start": { "dateTime": "2026-10-21T10:00:00", "timeZone": "UTC" }, "end": { "dateTime": "2026-10-21T11:00:00", "timeZone": "UTC" } })).await;
    assert_eq!(s, 403);
    assert_eq!(v["error"]["code"], "ErrorAccessDenied");
    let now = chrono::Utc::now();
    let from = (now - chrono::Duration::days(7)).format("%Y-%m-%dT00:00:00Z");
    let to = (now + chrono::Duration::days(14)).format("%Y-%m-%dT00:00:00Z");
    let mut total = 0;
    for c in &cals {
        let items = t.get_all(&format!("/me/calendars/{}/calendarView?startDateTime={from}&endDateTime={to}", c["id"].as_str().unwrap()), &[UTC]).await;
        total += items.len();
    }
    assert!(total >= 15, "only {total} events");
    let holidays = t.get_all(&format!("/me/calendars/{hid}/calendarView?startDateTime=2026-01-01T00:00:00Z&endDateTime=2027-01-01T00:00:00Z"), &[]).await;
    assert!(holidays.iter().any(|h| h["subject"] == "Tag der Deutschen Einheit" && h["start"]["dateTime"] == "2026-10-03T00:00:00.0000000"));
    assert!(holidays.iter().any(|h| h["subject"] == "Ostermontag" && h["start"]["dateTime"] == "2026-04-06T00:00:00.0000000"));
}

#[tokio::test]
async fn mailbox_settings_and_people() {
    let t = setup(true).await;
    let (s, ar) = t.get("/me/mailboxSettings/automaticRepliesSetting").await;
    assert_eq!(s, 200);
    assert_eq!(ar["status"], "disabled");
    let (s, v) = t
        .patch(
            "/me/mailboxSettings",
            json!({ "automaticRepliesSetting": {
                "status": "scheduled", "externalAudience": "contactsOnly",
                "internalReplyMessage": "<p>Bin im Urlaub</p>", "externalReplyMessage": "<p>Out of office</p>",
                "scheduledStartDateTime": { "dateTime": "2026-12-20T00:00:00", "timeZone": "UTC" },
                "scheduledEndDateTime": { "dateTime": "2027-01-05T00:00:00", "timeZone": "UTC" } } }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, ar) = t.get("/me/mailboxSettings/automaticRepliesSetting").await;
    assert_eq!(ar["status"], "scheduled");
    assert_eq!(ar["scheduledEndDateTime"]["dateTime"], "2027-01-05T00:00:00");
    assert_eq!(t.patch("/me/mailboxSettings", json!({ "automaticRepliesSetting": { "status": "sometimes" } })).await.0, 400);
    let (_, ms) = t.get("/me/mailboxSettings").await;
    assert_eq!(ms["timeZone"], "W. Europe Standard Time");

    let (_, p) = t.get("/me/people?$search=%22lau%22&$top=10").await;
    let people = p["value"].as_array().unwrap();
    assert_eq!(people.len(), 1);
    assert_eq!(people[0]["scoredEmailAddresses"][0]["address"], "laura.schmidt@contoso.example");
    let (_, p) = t.get("/me/people?$search=schmidt").await;
    assert_eq!(p["value"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn failure_injection() {
    let t = setup(false).await;
    t.srv.state.fail_next(429, 2);
    let r = t.c.get(t.url("/me")).bearer_auth(&t.tok).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 429);
    assert_eq!(r.headers()["retry-after"], "1");
    assert_eq!(t.get("/me").await.0, 429);
    assert_eq!(t.get("/me").await.0, 200);
    assert!(t.srv.state.request_count() >= 3);
}
