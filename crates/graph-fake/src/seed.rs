//! Initial tenant: mailboxes and (optionally) German demo content.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use chrono_tz::Europe::Berlin;
use serde_json::{json, Value};

use crate::calendar::{from_utc, new_event};
use crate::mail::seed_msg;
use crate::misc::person;
use crate::state::{Att, AuthState, Cal, Folder, Mailbox, Msg, State};
use crate::util::{email, graph_dt, new_id};
use crate::FakeOptions;

pub const PRIMARY: (&str, &str) = ("Max Mustermann", "max.mustermann@contoso.example");
pub const SHARED: (&str, &str) = ("Datenschutz Contoso", "datenschutz@contoso.example");

const ERIKA: (&str, &str) = ("Erika Beispiel", "erika.beispiel@fabrikam.example");
const JONAS: (&str, &str) = ("Jonas Weber", "jonas.weber@contoso.example");
const LAURA: (&str, &str) = ("Laura Schmidt", "laura.schmidt@contoso.example");
const THOMAS: (&str, &str) = ("Thomas Müller", "thomas.mueller@alpha-industries.example");
const SABINE: (&str, &str) = ("Sabine Keller", "sabine.keller@contoso.example");
const MICHAEL: (&str, &str) = ("Michael Braun", "michael.braun@northwind.example");
const ANNA: (&str, &str) = ("Anna Fischer", "anna.fischer@contoso.example");
const PETER: (&str, &str) = ("Peter Wagner", "peter.wagner@steuerberatung-wagner.example");
const JULIA: (&str, &str) = ("Julia Hoffmann", "julia.hoffmann@contoso.example");
const DANIEL: (&str, &str) = ("Daniel Schulz", "daniel.schulz@tailspin.example");
const KATRIN: (&str, &str) = ("Katrin Becker", "katrin.becker@contoso.example");
const IT: (&str, &str) = ("IT-Service Contoso", "it-service@contoso.example");
const NEWS: (&str, &str) = ("Digital Business Weekly", "newsletter@digital-business-weekly.example");

pub fn build(base_url: &str, opts: &FakeOptions) -> State {
    let mut primary = Mailbox::new(PRIMARY.1, PRIMARY.0, true);
    let mut shared = Mailbox::new(SHARED.1, SHARED.0, false);
    if opts.seed_demo {
        seed_primary(&mut primary, opts.now);
        seed_shared(&mut shared, opts.now);
    }
    State {
        base_url: base_url.to_string(),
        mailboxes: vec![primary, shared],
        auth: AuthState::default(),
        uploads: HashMap::new(),
        faults: Default::default(),
        request_count: 0,
        token_requests: 0,
        refresh_revoked: false,
    }
}

fn html(paras: &[&str], signature: &str) -> String {
    let mut s = String::from(
        "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"></head>\
<body style=\"font-family:Calibri,Arial,sans-serif;font-size:11pt\">",
    );
    for p in paras {
        if p.starts_with('<') {
            s.push_str(p);
        } else {
            s.push_str(&format!("<p>{p}</p>"));
        }
    }
    if !signature.is_empty() {
        s.push_str(&format!("<p>Viele Grüße<br>{signature}</p>"));
    }
    s.push_str("</body></html>");
    s
}

fn add_folder(mb: &mut Mailbox, name: &str, parent: &str) -> String {
    let id = new_id("AAMkAF");
    let parent = mb.folder_id(parent).unwrap_or_else(|| parent.to_string());
    mb.folders.push(Folder { id: id.clone(), display_name: name.to_string(), parent_id: parent, well_known: None });
    id
}

fn hours(h: f64) -> Duration {
    Duration::minutes((h * 60.0) as i64)
}

fn me_to(to: &[(&str, &str)]) -> Value {
    json!({ "from": email(PRIMARY.0, PRIMARY.1), "toRecipients": to.iter().map(|p| email(p.0, p.1)).collect::<Vec<_>>(), "isRead": true })
}

fn seed_primary(mb: &mut Mailbox, now: DateTime<Utc>) {
    let projekte = add_folder(mb, "Projekte", "inbox");
    let alpha = add_folder(mb, "Kunde Alpha", &projekte);
    let rechnungen = add_folder(mb, "Rechnungen", "inbox");

    mb.categories = vec![
        json!({ "id": uuid::Uuid::new_v4().to_string(), "displayName": "Wichtig", "color": "preset0" }),
        json!({ "id": uuid::Uuid::new_v4().to_string(), "displayName": "Business & Pitch", "color": "preset7" }),
        json!({ "id": uuid::Uuid::new_v4().to_string(), "displayName": "Info", "color": "preset4" }),
        json!({ "id": uuid::Uuid::new_v4().to_string(), "displayName": "Privat", "color": "preset8" }),
        json!({ "id": uuid::Uuid::new_v4().to_string(), "displayName": "Rechnung", "color": "preset1" }),
        json!({ "id": uuid::Uuid::new_v4().to_string(), "displayName": "Warten auf Antwort", "color": "preset3" }),
    ];

    mb.people = [
        (ERIKA, "Fabrikam AG", "Einkauf"),
        (JONAS, "Contoso GmbH", "Vertrieb"),
        (LAURA, "Contoso GmbH", "Projektleitung"),
        (THOMAS, "Alpha Industries", "CTO"),
        (SABINE, "Contoso GmbH", "Buchhaltung"),
        (MICHAEL, "Northwind Traders", "Geschäftsführer"),
        (ANNA, "Contoso GmbH", "Personal"),
        (PETER, "Steuerberatung Wagner", "Steuerberater"),
        (JULIA, "Contoso GmbH", "Marketing"),
        (DANIEL, "Tailspin Toys", "Einkauf"),
        (KATRIN, "Contoso GmbH", "IT"),
        (SHARED, "Contoso GmbH", "Datenschutz"),
    ]
    .iter()
    .map(|(p, c, t)| person(p.0, p.1, c, t))
    .collect();
    mb.contacts = mb
        .people
        .iter()
        .filter(|p| p["jobTitle"] != "Datenschutz")
        .map(|p| (0, crate::contacts::from_person(p)))
        .collect();

    // ---- calendars & events (first, so the invitation can link to its event)
    mb.calendars.push(Cal { id: new_id("AAMkAC"), name: "Feiertage".into(), color: "lightGreen".into(), hex_color: "#2e7d32".into(), can_edit: false, is_default: false });
    mb.calendars.push(Cal { id: new_id("AAMkAC"), name: "Team".into(), color: "lightPurple".into(), hex_color: "#8e24aa".into(), can_edit: true, is_default: false });
    let invite_event = seed_events(mb, now);

    let pdf = crate::png::tiny_pdf("Rechnung 2026-1043 - Fabrikam AG - 4.284,00 EUR");
    let banner = crate::png::banner();
    let conv_offer = new_id("AAQkAD");
    let conv_alpha = new_id("AAQkAD");

    let add = |mb: &mut Mailbox, folder: &str, from: (&str, &str), subject: &str, body: String, h: f64, extra: Value| -> String {
        let m = seed_msg(mb, folder, from, subject, &body, now - hours(h), extra);
        mb.insert(m)
    };

    // ---------------------------------------------------------- inbox
    add(mb, "inbox", LAURA, "Re: Einziger Fokus müsste sein – agentische Umsetzung", html(&[
        "Hallo Max,",
        "ich sehe das genauso: Wir sollten uns im nächsten Quartal voll auf die agentische Umsetzung konzentrieren. Die anderen Themen parken wir bis Februar.",
        "Kannst du bis Donnerstag eine kurze Priorisierung schicken? Dann stimme ich das mit Jonas ab.",
    ], "Laura"), 0.6, json!({ "isRead": false, "categories": ["Info"], "flag": { "flagStatus": "flagged" }, "ccRecipients": [email(JONAS.0, JONAS.1)] }));

    add(mb, "inbox", THOMAS, "Re: Anforderungen Schnittstelle ERP", html(&[
        "Hallo Herr Mustermann,",
        "anbei wie besprochen die Übersicht der benötigten Felder:",
        "<table border=\"1\" cellpadding=\"4\" cellspacing=\"0\" style=\"border-collapse:collapse\"><tr style=\"background:#e7eef7\"><th>Feld</th><th>Typ</th><th>Pflicht</th></tr><tr><td>Kundennummer</td><td>String(10)</td><td>ja</td></tr><tr><td>Auftragsdatum</td><td>Datum</td><td>ja</td></tr><tr><td>Lieferadresse</td><td>Adresse</td><td>nein</td></tr><tr><td>Positionen</td><td>Liste</td><td>ja</td></tr></table>",
        "Die Dokumentation der API finden Sie unter <a href=\"https://alpha-industries.example/api/docs\">alpha-industries.example/api/docs</a>.",
    ], "Thomas Müller<br>CTO, Alpha Industries"), 1.5, json!({ "isRead": false, "importance": "high", "categories": ["Business & Pitch"], "conversationId": conv_alpha }));

    let id = add(mb, "inbox", MICHAEL, "Einladung: Präsentation Angebot Northwind", html(&[
        "Hallo Max,",
        "ich lade dich zur Präsentation unseres Angebots ein. Bitte gib kurz Bescheid, ob der Termin passt.",
    ], "Michael Braun"), 2.2, json!({
        "isRead": false,
        "@odata.type": "#microsoft.graph.eventMessageRequest",
        "meetingMessageType": "meetingRequest",
        "meetingRequestType": "newMeetingRequest",
        "isOutOfDate": false,
        "isAllDay": false,
        "isDelegated": false,
        "responseRequested": true,
        "allowNewTimeProposals": true,
        "type": "singleInstance",
        "location": { "displayName": "Microsoft Teams-Besprechung" },
    }));
    if let Some((eid, start, end)) = &invite_event {
        let m = mb.messages.get_mut(&id).expect("inserted");
        m.event_id = Some(eid.clone());
        m.json["startDateTime"] = json!({ "dateTime": graph_dt(*start), "timeZone": "UTC" });
        m.json["endDateTime"] = json!({ "dateTime": graph_dt(*end), "timeZone": "UTC" });
    }

    let id = add(mb, "inbox", ERIKA, "Rechnung 2026-1043", html(&[
        "Sehr geehrter Herr Mustermann,",
        "anbei erhalten Sie unsere Rechnung Nr. 2026-1043 über <b>4.284,00 €</b> für die Lizenzen im Oktober.",
        "Bitte überweisen Sie den Betrag innerhalb von 14 Tagen.",
    ], "Erika Beispiel<br>Fabrikam AG – Einkauf &amp; Abrechnung"), 4.0, json!({ "isRead": true, "categories": ["Rechnung"] }));
    mb.messages.get_mut(&id).expect("inserted").attachments.push(Att::new("Rechnung_2026-1043.pdf", "application/pdf", pdf.clone(), false, None));

    let id = add(mb, "inbox", JULIA, "Neues Logo für den Website-Relaunch", html(&[
        "Hi Max,",
        "hier der finale Entwurf für den Header der neuen Website:",
        "<p><img src=\"cid:banner001@contoso.example\" alt=\"Banner\" width=\"240\" height=\"48\"></p>",
        "Feedback gerne bis Freitag. Das Review ist heute Nachmittag.",
    ], "Julia<br>Marketing"), 5.5, json!({ "isRead": false, "categories": ["Info"] }));
    mb.messages.get_mut(&id).expect("inserted").attachments.push(Att::new("image001.png", "image/png", banner.clone(), true, Some("banner001@contoso.example")));

    add(mb, "inbox", KATRIN, "Wartungsfenster Mailserver am Samstag", html(&[
        "Liebe Kolleginnen und Kollegen,",
        "am Samstag zwischen 06:00 und 10:00 Uhr finden Wartungsarbeiten statt. In dieser Zeit kann es zu kurzen Unterbrechungen kommen.",
    ], "Katrin Becker<br>IT"), 7.0, json!({ "isRead": true }));

    add(mb, "inbox", PETER, "Unterlagen Jahresabschluss", "Hallo Herr Mustermann,\r\n\r\nfür den Jahresabschluss benötige ich noch:\r\n\r\n- Inventurliste zum 31.12.\r\n- Kontoauszüge Dezember\r\n- Darlehensverträge (falls neu)\r\n\r\nMit freundlichen Grüßen\r\nPeter Wagner\r\nSteuerberatung Wagner".to_string(), 26.0,
        json!({ "isRead": false, "body": { "contentType": "text", "content": "Hallo Herr Mustermann,\r\n\r\nfür den Jahresabschluss benötige ich noch:\r\n\r\n- Inventurliste zum 31.12.\r\n- Kontoauszüge Dezember\r\n- Darlehensverträge (falls neu)\r\n\r\nMit freundlichen Grüßen\r\nPeter Wagner\r\nSteuerberatung Wagner" }, "flag": { "flagStatus": "flagged" } }));

    add(mb, "inbox", JONAS, "Re: Angebot Tailspin Toys", html(&[
        "Hi Max,",
        "Tailspin hat positiv reagiert. Sie wünschen sich aber noch 5 % Rabatt bei 3 Jahren Laufzeit. Wie stehst du dazu?",
    ], "Jonas"), 28.0, json!({ "isRead": true, "categories": ["Business & Pitch", "Warten auf Antwort"], "conversationId": conv_offer }));

    add(mb, "inbox", ANNA, "Erinnerung: Zielvereinbarungsgespräche", html(&[
        "Hallo Max,",
        "bitte denk daran, die Zielvereinbarungsgespräche mit deinem Team bis Ende des Monats zu terminieren. Die Vorlage liegt im Intranet.",
    ], "Anna Fischer<br>Personal"), 30.0, json!({ "isRead": true, "flag": { "flagStatus": "complete" } }));

    add(mb, "inbox", NEWS, "Digital Business Weekly: KI-Agenten im Mittelstand", html(&[
        "<div style=\"background:#0b3d91;color:white;padding:12px;font-size:20px\">Digital Business Weekly</div>",
        "<p><img src=\"https://picsum.photos/seed/contoso/600/160\" alt=\"Titelbild\" width=\"600\" height=\"160\"></p>",
        "<h2>KI-Agenten im Mittelstand: Was jetzt zählt</h2>",
        "Immer mehr Unternehmen setzen auf autonome Software-Agenten. Wir zeigen, welche Anwendungsfälle sich bereits rechnen.",
        "<p><a href=\"https://digital-business-weekly.example/artikel/ki-agenten\">Weiterlesen</a> · <a href=\"https://digital-business-weekly.example/abmelden\">Abmelden</a></p>",
    ], ""), 33.0, json!({ "isRead": false, "inferenceClassification": "other" }));

    add(mb, "inbox", DANIEL, "Bestellung 4711 – Liefertermin", html(&[
        "Guten Tag Herr Mustermann,",
        "können Sie den Liefertermin für Bestellung 4711 bestätigen? Wir planen die Auslieferung an unsere Filialen für KW 46.",
    ], "Daniel Schulz<br>Tailspin Toys"), 50.0, json!({ "isRead": true }));

    add(mb, "inbox", SABINE, "Reisekostenabrechnung September", html(&[
        "Hallo Max,",
        "deine Reisekostenabrechnung für September ist geprüft und wird mit dem nächsten Gehaltslauf ausgezahlt.",
    ], "Sabine Keller<br>Buchhaltung"), 75.0, json!({ "isRead": true, "categories": ["Rechnung"] }));

    add(mb, "inbox", LAURA, "Protokoll Lenkungskreis", html(&[
        "Hallo zusammen,",
        "anbei das Protokoll des Lenkungskreises. Wichtigste Entscheidungen:",
        "<ul><li>Go-Live Portal: 1. Dezember</li><li>Budgetfreigabe Phase 2</li><li>Neues Reporting ab Januar</li></ul>",
    ], "Laura"), 98.0, json!({ "isRead": true, "categories": ["Info"], "ccRecipients": [email(JONAS.0, JONAS.1), email(KATRIN.0, KATRIN.1)] }));

    add(mb, "inbox", MICHAEL, "Kurze Frage zum Rahmenvertrag", html(&[
        "Hallo Max,",
        "gilt die Preisgleitklausel aus §7 auch für Zusatzmodule? Ich habe das im Vertrag nicht eindeutig gefunden.",
    ], "Michael"), 120.0, json!({ "isRead": false, "importance": "high", "categories": ["Wichtig"] }));

    add(mb, "inbox", THOMAS, "Testzugang Staging", html(&[
        "Hallo Herr Mustermann,",
        "der Testzugang zum Staging-System ist eingerichtet. Zugangsdaten folgen separat.",
    ], "Thomas Müller"), 145.0, json!({ "isRead": true, "conversationId": conv_alpha }));

    add(mb, "inbox", JULIA, "Social-Media-Plan November", html(&[
        "Hi Max,",
        "anbei der Redaktionsplan für November. Bitte bis Mittwoch freigeben.",
    ], "Julia"), 170.0, json!({ "isRead": true, "flag": { "flagStatus": "flagged" } }));

    add(mb, "inbox", KATRIN, "Neue Laptops sind da", html(&["Hallo Max,", "die neuen Laptops für dein Team können ab Montag in der IT abgeholt werden."], "Katrin"), 200.0, json!({ "isRead": true }));

    add(mb, "inbox", ERIKA, "Lizenzverlängerung 2027", html(&[
        "Sehr geehrter Herr Mustermann,",
        "Ihre Lizenzen laufen zum 31.12. aus. Gerne unterbreiten wir Ihnen ein Angebot für die Verlängerung.",
    ], "Erika Beispiel"), 260.0, json!({ "isRead": true, "categories": ["Business & Pitch"] }));

    add(mb, "inbox", JONAS, "Messeplanung Hannover", html(&["Hi Max,", "Standplan und Schichtplan für die Messe sind im Teams-Kanal. Bitte trag dich ein."], "Jonas"), 300.0, json!({ "isRead": true }));

    add(mb, "inbox", ANNA, "Willkommen an Bord: neue Kollegin im Vertrieb", html(&["Liebe Kolleginnen und Kollegen,", "ab dem 1. verstärkt Frau Lea Wolf unser Vertriebsteam. Herzlich willkommen!"], "Anna Fischer"), 340.0, json!({ "isRead": true, "categories": ["Info"] }));

    add(mb, "inbox", PETER, "Umsatzsteuervoranmeldung Q3", html(&["Hallo Herr Mustermann,", "die Umsatzsteuervoranmeldung für Q3 wurde übermittelt."], "Peter Wagner"), 400.0, json!({ "isRead": true, "categories": ["Rechnung"] }));

    add(mb, "inbox", DANIEL, "Rückfrage Artikelstammdaten", html(&["Guten Tag,", "uns fehlen für 12 Artikel noch die EAN-Codes. Liste anbei im Text:", "<pre>10023, 10024, 10031, 10040, 10041, 10055, 10056, 10060, 10061, 10077, 10078, 10090</pre>"], "Daniel Schulz"), 450.0, json!({ "isRead": true }));

    add(mb, "inbox", LAURA, "Urlaubsplanung Weihnachten", html(&["Hallo Max,", "bitte trag deinen Urlaub zwischen den Jahren bis zum 15.11. in den Teamkalender ein."], "Laura"), 500.0, json!({ "isRead": true, "categories": ["Privat"] }));

    add(mb, "inbox", NEWS, "Digital Business Weekly: Cloud-Kosten im Griff", html(&["<h2>Cloud-Kosten im Griff</h2>", "Fünf Hebel, mit denen Sie Ihre Cloud-Rechnung senken."], ""), 530.0, json!({ "isRead": true, "inferenceClassification": "other" }));

    add(mb, "inbox", MICHAEL, "Feedback zum Workshop", html(&["Hallo Max,", "vielen Dank für den gelungenen Workshop! Das Team war begeistert."], "Michael"), 600.0, json!({ "isRead": true, "flag": { "flagStatus": "complete" } }));

    add(mb, "inbox", SABINE, "Offene Posten Oktober", html(&["Hallo Max,", "folgende Kunden sind im Zahlungsverzug:", "<table border=\"1\" cellpadding=\"4\" style=\"border-collapse:collapse\"><tr><th>Kunde</th><th>Betrag</th><th>Tage</th></tr><tr><td>Tailspin Toys</td><td>12.400,00 €</td><td>21</td></tr><tr><td>Northwind Traders</td><td>3.150,00 €</td><td>9</td></tr></table>"], "Sabine"), 700.0, json!({ "isRead": true, "importance": "high", "categories": ["Rechnung", "Wichtig"] }));

    add(mb, "inbox", KATRIN, "Passwort-Richtlinie ab 1. November", html(&["Hallo zusammen,", "ab dem 1. November gilt eine Mindestlänge von 14 Zeichen. Bitte aktiviert zusätzlich die Mehrstufige Authentifizierung."], "Katrin"), 800.0, json!({ "isRead": true }));

    // filler notifications
    for i in 0..18u32 {
        let h = 30.0 + (i as f64) * 52.0 + 11.0;
        let (from, subj, text) = match i % 3 {
            0 => (IT, format!("Ticket #{} wurde aktualisiert", 48210 + i * 7), format!("Ihr Ticket #{} hat einen neuen Status: <b>In Bearbeitung</b>.", 48210 + i * 7)),
            1 => (JONAS, format!("Wochenbericht Vertrieb KW {}", 42 - (i / 3)), "Anbei die Zahlen der Woche: 4 neue Leads, 2 Angebote, 1 Abschluss.".to_string()),
            _ => (IT, format!("Freigabe erforderlich: Softwareanfrage {}", 3300 + i), "Eine neue Softwareanfrage wartet auf Ihre Freigabe.".to_string()),
        };
        add(mb, "inbox", from, &subj, html(&["Hallo Max,", &text], from.0), h, json!({ "isRead": i % 4 != 0 }));
    }

    // ---------------------------------------------------------- subfolders
    add(mb, &projekte, LAURA, "Projektstatus Portal", html(&["Hallo Max,", "Statusampel: grün. Nächster Meilenstein: UAT am 15.11."], "Laura"), 90.0, json!({ "isRead": true, "categories": ["Info"] }));
    add(mb, &projekte, JONAS, "Ressourcenplanung Q1", html(&["Hi Max,", "wir brauchen ab Januar zwei zusätzliche Entwickler für das Portal."], "Jonas"), 210.0, json!({ "isRead": false }));
    add(mb, &projekte, KATRIN, "Serverkosten Projekt Portal", html(&["Hallo Max,", "die Hosting-Kosten liegen aktuell bei 1.240 € pro Monat."], "Katrin"), 380.0, json!({ "isRead": true }));
    add(mb, &alpha, THOMAS, "Kick-off Alpha – Agenda", html(&["Hallo Herr Mustermann,", "anbei die Agenda für unser Kick-off:", "<ol><li>Vorstellung</li><li>Zielbild</li><li>Zeitplan</li><li>Nächste Schritte</li></ol>"], "Thomas Müller"), 400.0, json!({ "isRead": true, "conversationId": conv_alpha }));
    add(mb, &alpha, THOMAS, "Vertrag Alpha unterschrieben", html(&["Hallo Herr Mustermann,", "der Vertrag ist unterschrieben zurück. Wir freuen uns auf die Zusammenarbeit!"], "Thomas Müller"), 520.0, json!({ "isRead": true, "categories": ["Business & Pitch"], "flag": { "flagStatus": "complete" } }));
    add(mb, &alpha, LAURA, "Alpha: Teamaufstellung", html(&["Hi Max,", "für Alpha schlage ich Jonas als Account Lead und mich als PL vor."], "Laura"), 530.0, json!({ "isRead": false }));
    let id = add(mb, &rechnungen, ERIKA, "Rechnung 2026-0987", html(&["Sehr geehrter Herr Mustermann,", "anbei die Rechnung für September."], "Erika Beispiel"), 750.0, json!({ "isRead": true, "categories": ["Rechnung"] }));
    mb.messages.get_mut(&id).expect("inserted").attachments.push(Att::new("Rechnung_2026-0987.pdf", "application/pdf", crate::png::tiny_pdf("Rechnung 2026-0987"), false, None));
    add(mb, &rechnungen, PETER, "Honorarrechnung Q3", html(&["Hallo Herr Mustermann,", "anbei meine Honorarrechnung für das dritte Quartal."], "Peter Wagner"), 900.0, json!({ "isRead": true, "categories": ["Rechnung"] }));
    add(mb, &rechnungen, SABINE, "Zahlungslauf Freitag", html(&["Hallo Max,", "bitte gib bis Donnerstag den Zahlungslauf frei."], "Sabine"), 120.0, json!({ "isRead": false, "flag": { "flagStatus": "flagged" } }));

    // ---------------------------------------------------------- sent items
    let sent = [
        (JONAS, "Angebot Tailspin Toys", "anbei das Angebot für Tailspin. Bitte prüfe die Konditionen auf Seite 3.", 30.0, Some(conv_offer.clone())),
        (THOMAS, "Anforderungen Schnittstelle ERP", "könnten Sie mir die Liste der benötigten Felder schicken?", 50.0, Some(conv_alpha.clone())),
        (LAURA, "Einziger Fokus müsste sein – agentische Umsetzung", "ich würde vorschlagen, dass wir uns auf die agentische Umsetzung konzentrieren.", 3.0, None),
        (ERIKA, "Bestellung Lizenzen", "wir benötigen 25 zusätzliche Lizenzen ab November.", 160.0, None),
        (MICHAEL, "Re: Feedback zum Workshop", "vielen Dank für das Feedback, das freut uns sehr!", 590.0, None),
        (PETER, "Belege Oktober", "anbei die Belege für Oktober.", 230.0, None),
        (DANIEL, "Re: Bestellung 4711 – Liefertermin", "der Liefertermin KW 46 ist bestätigt.", 48.0, None),
        (JULIA, "Texte für die Startseite", "hier meine Vorschläge für die Texte auf der Startseite.", 120.0, None),
    ];
    for (to, subj, text, h, conv) in sent {
        let mut extra = me_to(&[to]);
        if let Some(c) = conv {
            extra["conversationId"] = json!(c);
        }
        let first = to.0.split(' ').next().unwrap_or("");
        add(mb, "sentitems", PRIMARY, subj, html(&[&format!("Hallo {first},"), text], "Max"), h, extra);
    }

    // ---------------------------------------------------------- drafts / deleted / archive / junk
    let mut d = me_to(&[MICHAEL]);
    d["isDraft"] = json!(true);
    add(mb, "drafts", PRIMARY, "Entwurf: Antwort Rahmenvertrag", html(&["Hallo Michael,", "zur Preisgleitklausel: "], ""), 20.0, d);

    add(mb, "deleteditems", NEWS, "Digital Business Weekly: Sonderausgabe", html(&["<h2>Sonderausgabe</h2>", "Die Top-Themen des Monats."], ""), 100.0, json!({ "isRead": true }));
    add(mb, "deleteditems", IT, "Ticket #48100 geschlossen", html(&["Ihr Ticket wurde geschlossen."], ""), 300.0, json!({ "isRead": true }));
    add(mb, "deleteditems", JONAS, "Mittagessen?", html(&["Lust auf Pizza um 12?"], "Jonas"), 150.0, json!({ "isRead": true }));

    for (i, (from, subj)) in [
        (ERIKA, "Rahmenvertrag 2025"),
        (THOMAS, "Erstgespräch Alpha"),
        (MICHAEL, "Northwind – Kennenlernen"),
        (SABINE, "Jahresabschluss 2025"),
        (ANNA, "Betriebsausflug 2025"),
    ]
    .into_iter()
    .enumerate()
    {
        add(mb, "archive", from, subj, html(&["Archivierte Nachricht.", "Details siehe Anhang bzw. Verlauf."], from.0), 1000.0 + i as f64 * 200.0, json!({ "isRead": true }));
    }

    add(mb, "junkemail", ("Gewinnspiel-Team", "win@lucky-prize.example"), "Herzlichen Glückwunsch! Sie haben gewonnen!!!", html(&["Klicken Sie <a href=\"https://lucky-prize.example\">hier</a>, um Ihren Preis abzuholen."], ""), 40.0, json!({ "isRead": false }));
    add(mb, "junkemail", ("Paket Service", "noreply@paket-zustellung.example"), "Ihre Sendung konnte nicht zugestellt werden", html(&["Bitte bestätigen Sie Ihre Adresse."], ""), 140.0, json!({ "isRead": false }));
}

/// Creates demo events; returns (id, start utc, end utc) of the invitation event.
fn seed_events(mb: &mut Mailbox, now: DateTime<Utc>) -> Option<(String, chrono::NaiveDateTime, chrono::NaiveDateTime)> {
    let default_cal = mb.default_calendar_id();
    let team = mb.calendars.iter().find(|c| c.name == "Team").map(|c| c.id.clone()).unwrap_or_default();
    let holidays = mb.calendars.iter().find(|c| c.name == "Feiertage").map(|c| c.id.clone()).unwrap_or_default();
    let today = from_utc(now.naive_utc(), Berlin).date();
    let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let day = |offset: i64| monday + Duration::days(offset);
    let dt = |d: NaiveDate, h: u32, m: u32| json!({ "dateTime": format!("{}T{:02}:{:02}:00", d.format("%Y-%m-%d"), h, m), "timeZone": "Europe/Berlin" });
    let allday = |d: NaiveDate| json!({ "dateTime": format!("{}T00:00:00", d.format("%Y-%m-%d")), "timeZone": "Europe/Berlin" });
    let att = |p: (&str, &str), response: &str, ty: &str| json!({ "type": ty, "emailAddress": { "name": p.0, "address": p.1 }, "status": { "response": response, "time": "2026-01-01T00:00:00Z" } });

    let add = |mb: &mut Mailbox, cal: &str, body: Value, patch: Value| -> Option<String> {
        let mut ev = new_event(mb, cal, &body).ok()?;
        crate::util::merge(&mut ev.json, &patch);
        let id = ev.id.clone();
        mb.events.insert(id.clone(), ev);
        Some(id)
    };

    add(mb, &default_cal, json!({
        "subject": "Jour fixe Geschäftsleitung",
        "start": dt(day(0), 9, 30), "end": dt(day(0), 10, 30),
        "location": { "displayName": "Besprechungsraum Elbe" },
        "attendees": [att(JONAS, "accepted", "required"), att(LAURA, "accepted", "required"), att(KATRIN, "tentativelyAccepted", "optional")],
        "categories": ["Info"],
        "recurrence": { "pattern": { "type": "weekly", "interval": 1, "daysOfWeek": ["monday"], "firstDayOfWeek": "monday" },
                        "range": { "type": "noEnd", "startDate": day(-56).format("%Y-%m-%d").to_string(), "recurrenceTimeZone": "Europe/Berlin" } },
        "body": { "contentType": "html", "content": "<p>Agenda: Zahlen, Projekte, Personal, Sonstiges</p>" },
    }), json!({}));

    add(mb, &team, json!({
        "subject": "Daily Stand-up",
        "start": dt(day(-14), 9, 0), "end": dt(day(-14), 9, 15),
        "isOnlineMeeting": true,
        "location": { "displayName": "Microsoft Teams-Besprechung" },
        "attendees": [att(JONAS, "accepted", "required"), att(LAURA, "accepted", "required"), att(JULIA, "none", "required")],
        "recurrence": { "pattern": { "type": "weekly", "interval": 1, "daysOfWeek": ["monday", "tuesday", "wednesday", "thursday", "friday"], "firstDayOfWeek": "monday" },
                        "range": { "type": "endDate", "startDate": day(-14).format("%Y-%m-%d").to_string(), "endDate": day(30).format("%Y-%m-%d").to_string(), "recurrenceTimeZone": "Europe/Berlin" } },
        "reminderMinutesBeforeStart": 5,
    }), json!({}));

    add(mb, &default_cal, json!({
        "subject": "Monatsabschluss Buchhaltung",
        "start": dt(day(-90), 10, 0), "end": dt(day(-90), 11, 0),
        "attendees": [att(SABINE, "accepted", "required")],
        "categories": ["Rechnung"],
        "recurrence": { "pattern": { "type": "absoluteMonthly", "interval": 1, "dayOfMonth": 3 },
                        "range": { "type": "noEnd", "startDate": day(-90).format("%Y-%m-%d").to_string() } },
    }), json!({}));

    let bday = NaiveDate::from_ymd_opt(today.year() - 1, 11, 4).unwrap_or(today);
    add(mb, &default_cal, json!({
        "subject": "Geburtstag Laura Schmidt",
        "isAllDay": true, "start": allday(bday), "end": allday(bday + Duration::days(1)),
        "showAs": "free", "categories": ["Privat"],
        "recurrence": { "pattern": { "type": "absoluteYearly", "interval": 1, "dayOfMonth": 4, "month": 11 },
                        "range": { "type": "noEnd", "startDate": bday.format("%Y-%m-%d").to_string() } },
    }), json!({}));

    add(mb, &default_cal, json!({
        "subject": "Kundentermin Alpha – Anforderungen",
        "start": dt(day(1), 11, 0), "end": dt(day(1), 12, 0),
        "location": { "displayName": "Alpha Industries, Leopoldstraße 10, München" },
        "attendees": [att(THOMAS, "accepted", "required"), att(LAURA, "tentativelyAccepted", "required")],
        "categories": ["Business & Pitch"],
        "body": { "contentType": "html", "content": "<p>Anforderungsworkshop ERP-Schnittstelle. Bitte Laptop mitbringen.</p>" },
    }), json!({}));

    add(mb, &default_cal, json!({
        "subject": "Budgetplanung Q1",
        "start": dt(day(2), 14, 0), "end": dt(day(2), 15, 0),
        "location": { "displayName": "Besprechungsraum Rhein" },
        "attendees": [att(SABINE, "accepted", "required"), att(JONAS, "declined", "optional")],
        "importance": "high",
        "categories": ["Wichtig"],
    }), json!({}));

    add(mb, &default_cal, json!({
        "subject": "Telefonat Steuerberater",
        "start": dt(day(2), 14, 30), "end": dt(day(2), 15, 30),
        "location": { "displayName": "Telefon" },
        "attendees": [att(PETER, "accepted", "required")],
    }), json!({}));

    add(mb, &default_cal, json!({
        "subject": "Messebesuch Hannover",
        "isAllDay": true, "start": allday(day(3)), "end": allday(day(5)),
        "showAs": "oof", "location": { "displayName": "Messegelände Hannover" },
    }), json!({}));

    add(mb, &default_cal, json!({
        "subject": "Mittagessen mit Michael Braun",
        "start": dt(day(4), 12, 0), "end": dt(day(4), 13, 0),
        "location": { "displayName": "Trattoria Da Marco" },
        "categories": ["Privat"],
    }), json!({}));

    add(mb, &default_cal, json!({
        "subject": "Retrospektive Sprint 42",
        "start": dt(day(-5), 15, 0), "end": dt(day(-5), 16, 0),
        "attendees": [att(LAURA, "accepted", "required"), att(JONAS, "accepted", "required")],
    }), json!({}));

    add(mb, &default_cal, json!({
        "subject": "Review Website-Relaunch",
        "start": dt(today, 15, 0), "end": dt(today, 16, 0),
        "isOnlineMeeting": true,
        "location": { "displayName": "Microsoft Teams-Besprechung" },
        "attendees": [att(JULIA, "accepted", "required")],
        "categories": ["Info"],
    }), json!({}));

    add(mb, &default_cal, json!({
        "subject": "Pitch-Vorbereitung Northwind",
        "start": dt(day(7), 13, 0), "end": dt(day(7), 14, 0),
        "isOnlineMeeting": true,
        "attendees": [att(JONAS, "accepted", "required")],
        "categories": ["Business & Pitch"],
    }), json!({}));

    add(mb, &team, json!({
        "subject": "Workshop Datenschutz (DSGVO)",
        "start": dt(day(8), 10, 0), "end": dt(day(8), 11, 30),
        "location": { "displayName": "Schulungsraum 2" },
        "attendees": [att(ANNA, "accepted", "required"), att(KATRIN, "none", "required"), att(SHARED, "accepted", "resource")],
    }), json!({}));

    add(mb, &default_cal, json!({
        "subject": "Zahnarzt",
        "start": dt(day(9), 16, 0), "end": dt(day(9), 17, 0),
        "sensitivity": "private", "showAs": "oof", "categories": ["Privat"],
    }), json!({}));

    add(mb, &default_cal, json!({
        "subject": "Urlaub",
        "isAllDay": true, "start": allday(day(11)), "end": allday(day(12)),
        "showAs": "oof", "categories": ["Privat"],
    }), json!({}));

    add(mb, &team, json!({
        "subject": "Team-Event: Bowling",
        "start": dt(day(11), 18, 0), "end": dt(day(11), 21, 0),
        "location": { "displayName": "Bowling-Center Mitte" },
        "showAs": "free",
    }), json!({}));

    let invite = add(mb, &default_cal, json!({
        "subject": "Präsentation Angebot Northwind",
        "start": dt(day(10), 14, 0), "end": dt(day(10), 15, 0),
        "isOnlineMeeting": true,
        "location": { "displayName": "Microsoft Teams-Besprechung" },
        "attendees": [att(PRIMARY, "none", "required"), att(JONAS, "accepted", "optional")],
        "body": { "contentType": "html", "content": "<p>Vorstellung unseres Angebots für die Plattformmigration.</p>" },
        "organizer": email(MICHAEL.0, MICHAEL.1),
    }), json!({
        "isOrganizer": false,
        "showAs": "tentative",
        "responseStatus": { "response": "notResponded", "time": "0001-01-01T00:00:00Z" },
    }));

    // German public holidays ±1 year
    for year in [today.year() - 1, today.year(), today.year() + 1] {
        for (name, d) in holidays_of(year) {
            add(mb, &holidays, json!({
                "subject": name, "isAllDay": true, "start": allday(d), "end": allday(d + Duration::days(1)), "showAs": "free",
                "isReminderOn": false,
            }), json!({ "isOrganizer": false, "responseStatus": { "response": "none", "time": "0001-01-01T00:00:00Z" } }));
        }
    }

    invite.and_then(|id| mb.events.get(&id).map(|e| (id.clone(), e.start, e.end)))
}

fn easter(y: i32) -> NaiveDate {
    let a = y % 19;
    let b = y / 100;
    let c = y % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    NaiveDate::from_ymd_opt(y, month as u32, day as u32).expect("valid easter date")
}

fn holidays_of(y: i32) -> Vec<(&'static str, NaiveDate)> {
    let e = easter(y);
    let d = |m: u32, day: u32| NaiveDate::from_ymd_opt(y, m, day).expect("valid date");
    vec![
        ("Neujahr", d(1, 1)),
        ("Karfreitag", e - Duration::days(2)),
        ("Ostermontag", e + Duration::days(1)),
        ("Tag der Arbeit", d(5, 1)),
        ("Christi Himmelfahrt", e + Duration::days(39)),
        ("Pfingstmontag", e + Duration::days(50)),
        ("Tag der Deutschen Einheit", d(10, 3)),
        ("1. Weihnachtsfeiertag", d(12, 25)),
        ("2. Weihnachtsfeiertag", d(12, 26)),
    ]
}

fn seed_shared(mb: &mut Mailbox, now: DateTime<Utc>) {
    mb.categories = vec![
        json!({ "id": uuid::Uuid::new_v4().to_string(), "displayName": "Auskunftsersuchen", "color": "preset0" }),
        json!({ "id": uuid::Uuid::new_v4().to_string(), "displayName": "Erledigt", "color": "preset4" }),
    ];
    let me = mb.me();
    type Item<'a> = (&'a str, &'a str, &'a str, f64, bool, &'a [&'a str]);
    let items: [Item; 6] = [
        ("Hans Meier <hans.meier@mail.example>", "Auskunftsersuchen nach Art. 15 DSGVO", "Sehr geehrte Damen und Herren, hiermit bitte ich um Auskunft über die zu meiner Person gespeicherten Daten.", 3.0, false, &["Auskunftsersuchen"]),
        ("Anna Fischer <anna.fischer@contoso.example>", "Verarbeitungsverzeichnis aktualisieren", "Bitte das Verarbeitungsverzeichnis um das neue Bewerbertool ergänzen.", 27.0, false, &[]),
        ("Katrin Becker <katrin.becker@contoso.example>", "Meldung: verlorener Laptop", "Ein Mitarbeiter hat seinen Laptop im Zug verloren. Festplatte ist verschlüsselt.", 50.0, true, &[]),
        ("Datenschutzbehörde <poststelle@lfd.example>", "Rundschreiben: Neue Leitlinien KI", "Anbei die neuen Leitlinien zum Einsatz von KI-Systemen.", 100.0, true, &[]),
        ("Lisa Kraus <lisa.kraus@mail.example>", "Löschung meiner Daten", "Bitte löschen Sie alle meine Daten aus Ihrem Newsletter-Verteiler.", 180.0, true, &["Erledigt"]),
        ("Jonas Weber <jonas.weber@contoso.example>", "AV-Vertrag mit neuem Dienstleister", "Kannst du den AV-Vertrag mit dem neuen Hosting-Dienstleister prüfen?", 260.0, true, &[]),
    ];
    for (from, subject, text, h, read, cats) in items {
        let (name, addr) = from.split_once(" <").map(|(n, a)| (n, a.trim_end_matches('>'))).unwrap_or((from, from));
        let m: Msg = seed_msg(mb, "inbox", (name, addr), subject, &html(&["Guten Tag,", text], name), now - hours(h), json!({ "isRead": read, "categories": cats, "toRecipients": [me.clone()] }));
        mb.insert(m);
    }
    let m = seed_msg(mb, "sentitems", SHARED, "Re: Löschung meiner Daten", &html(&["Sehr geehrte Frau Kraus,", "Ihre Daten wurden gelöscht."], "Ihr Datenschutzteam"), now - hours(170.0), json!({ "toRecipients": [email("Lisa Kraus", "lisa.kraus@mail.example")] }));
    mb.insert(m);
}
