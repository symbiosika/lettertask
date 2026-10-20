//! Outlook contacts: `/me/contacts` CRUD and `/me/contacts/delta`.

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

use crate::state::Mailbox;
use crate::util::{apply_select, created, no_content, ok, GErr, GResult, Req};

const WRITABLE: &[&str] = &[
    "displayName", "givenName", "surname", "emailAddresses", "companyName", "jobTitle",
    "businessPhones", "mobilePhone", "homePhones", "personalNotes", "department",
];

pub fn handle(mb: &mut Mailbox, rest: &[&str], req: &Req) -> GResult {
    let m = &req.method;
    match rest {
        ["contacts"] if m == Method::GET => {
            let vals: Vec<Value> = mb.contacts.iter().map(|(_, c)| apply_select(c.clone(), req.q("select"))).collect();
            ok(crate::util::paged(req, vals, 10))
        }
        ["contacts", "delta"] if m == Method::GET => delta(mb, req),
        ["contacts"] if m == Method::POST => {
            let body = req.json()?;
            let mut c = json!({ "id": crate::util::new_id("AAMkADco"), "@odata.etag": "W/\"1\"" });
            merge(&mut c, &body)?;
            fill_display_name(&mut c);
            mb.seq += 1;
            mb.contacts.push((mb.seq, c.clone()));
            created(c)
        }
        ["contacts", id] => {
            let Some(pos) = mb.contacts.iter().position(|(_, c)| c["id"] == *id) else {
                return Err(GErr::not_found("contact"));
            };
            match *m {
                Method::GET => ok(apply_select(mb.contacts[pos].1.clone(), req.q("select"))),
                Method::PATCH => {
                    let body = req.json()?;
                    mb.seq += 1;
                    let seq = mb.seq;
                    let entry = &mut mb.contacts[pos];
                    merge(&mut entry.1, &body)?;
                    fill_display_name(&mut entry.1);
                    entry.0 = seq;
                    ok(entry.1.clone())
                }
                Method::DELETE => {
                    let (_, c) = mb.contacts.remove(pos);
                    mb.seq += 1;
                    mb.contact_tombstones.push((mb.seq, c["id"].as_str().unwrap_or("").to_string()));
                    no_content()
                }
                _ => Err(GErr::new(StatusCode::METHOD_NOT_ALLOWED, "MethodNotAllowed", "Unsupported method")),
            }
        }
        _ => Err(GErr::bad(format!("Unsupported request: {} /{}", req.method, rest.join("/")))),
    }
}

fn merge(c: &mut Value, body: &Value) -> Result<(), GErr> {
    let Some(obj) = body.as_object() else {
        return Err(GErr::bad("Body must be a JSON object."));
    };
    for (k, v) in obj {
        if !WRITABLE.contains(&k.as_str()) {
            return Err(GErr::bad(format!("Property '{k}' is read-only or unknown.")));
        }
        c[k] = v.clone();
    }
    Ok(())
}

fn fill_display_name(c: &mut Value) {
    if c["displayName"].as_str().unwrap_or("").is_empty() {
        let n = format!(
            "{} {}",
            c["givenName"].as_str().unwrap_or(""),
            c["surname"].as_str().unwrap_or("")
        );
        c["displayName"] = json!(n.trim());
    }
}

/// Delta: the token is the change sequence number at the time of the call.
fn delta(mb: &Mailbox, req: &Req) -> GResult {
    let since: Option<u64> = match req.q("deltatoken") {
        Some(t) => Some(t.parse().map_err(|_| GErr::bad("Invalid delta token"))?),
        None => None,
    };
    let mut vals: Vec<Value> = mb
        .contacts
        .iter()
        .filter(|(seq, _)| since.is_none_or(|s| *seq > s))
        .map(|(_, c)| apply_select(c.clone(), req.q("select")))
        .collect();
    if let Some(s) = since {
        for (seq, id) in &mb.contact_tombstones {
            if *seq > s {
                vals.push(json!({ "id": id, "@removed": { "reason": "deleted" } }));
            }
        }
    }
    ok(json!({
        "@odata.context": format!("{}/v1.0/$metadata#Collection(contact)", req.base_url),
        "value": vals,
        "@odata.deltaLink": format!("{}/v1.0/me/contacts/delta?$deltatoken={}", req.base_url, mb.seq),
    }))
}

/// Outlook contact built from a seeded `person`.
pub fn from_person(p: &Value) -> Value {
    let addr = p.pointer("/scoredEmailAddresses/0/address").and_then(Value::as_str).unwrap_or("");
    json!({
        "id": crate::util::new_id("AAMkADco"),
        "displayName": p["displayName"],
        "givenName": p["givenName"],
        "surname": p["surname"],
        "emailAddresses": [{ "name": p["displayName"], "address": addr }],
        "companyName": p["companyName"],
        "jobTitle": p["jobTitle"],
        "businessPhones": ["+49 30 1234 5678"],
        "mobilePhone": null,
        "homePhones": [],
        "personalNotes": "",
    })
}
