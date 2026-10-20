# Outlook Classic – fokussiertes Mailprogramm für Microsoft 365 und IMAP

Ein Desktop-Mailprogramm in **Rust** (Tauri 2) für konzentriertes Arbeiten nach dem
**Inbox-Zero-Prinzip**: Das Postfach ist die Aufgabenliste. Die Oberfläche orientiert sich an
eM Client – ruhig, schlicht, eine Mail = ein Eintrag (keine Konversations-Gruppierung).

Microsoft-365-Postfächer werden ausschließlich über die **Microsoft Graph API** angebunden
(EWS wird von Microsoft abgeschaltet), andere Anbieter über **IMAP/SMTP**.

## Funktionen

**E-Mail**
- Konten: **Microsoft 365** (Login im Browser inkl. MFA) und **IMAP/SMTP** mit automatischer
  Servererkennung (Thunderbird-Datenbank, autoconfig) und Verbindungstest
- Aktionsleiste: **Neu ▾** (E-Mail / Termin / Aufgabe), Antworten, Allen antworten, Weiterleiten,
  Archivieren, **Auf Wiedervorlage ▾**, Löschen
- Suche oben mit Bereich (aktueller Ordner / alle Ordner), Feld (Von, An, Betreff, Text),
  „Nur mit Anhang“ und letzten Suchen
- Liste mit Avatar, Vorschau und Tags; Lesebereich rechts, unten oder aus; HTML-Mails in einem
  **abgeschotteten WebView-iframe** (keine Skripte, CSP), externe Bilder erst auf Klick
- **Tags** (Outlook-Kategorien mit allen 25 Farben) oben rechts in der Mail – bis zu 20 direkt
  wählbar, Taste **T**
- Verfassen im eM-Client-Stil: Signatur ▾, Cc & Bcc, Rich-Text, Anhänge (auch > 3 MB),
  Adressvorschläge aus Kontakten
- **Signaturen pro Konto**: Liste mit Standard, einstellbar für Neu / Antworten / Weiterleiten
- **Kontakte**: Outlook-Kontakte werden lokal synchronisiert, Adressen von Empfängern automatisch
  gesammelt; eigener Kontakte-Bereich
- Abwesenheitsnotiz, Ordnerverwaltung, Drag & Drop, Offline-Lesen dank lokalem SQLite-Cache,
  Hintergrund-Synchronisation per Delta-Abfragen, Desktop-Benachrichtigungen

**Inbox Zero**
- **Auf Wiedervorlage**: Mail verschwindet in den Ordner „Wiedervorlage“ und kommt zum gewählten
  Zeitpunkt ungelesen zurück. Zeitpunkte konfigurierbar (Standard: Morgen 8 Uhr, Übermorgen,
  Am Wochenende, Nächste Woche, In 14 / 30 Tagen). Der Termin liegt auf dem Server
  (M365: Nachverfolgung, IMAP: Schlagwort) und gilt damit geräteübergreifend.
- **Aufgabe (Mail)**: Mail an sich selbst mit Fälligkeit („Sofort“ oder Wiedervorlage) – direkt
  serverseitig angelegt, erscheint auf keinem Gerät vorzeitig; optional **wiederkehrend**
  (täglich … jährlich, fester Rhythmus beim Archivieren)
- **Erinnerung an mich** beim Senden: Nachfass-Mail in der Wiedervorlage; sie **erledigt sich
  selbst**, sobald der Empfänger im selben Verlauf antwortet
- **Regeln vor der Anzeige**: Mails an sich selbst mit `#morgen`, `#heute`, `#wochenende`,
  `#nächstewoche`, `#14t`, `#2w` … im Betreff landen sofort auf Wiedervorlage (Aufgaben vom Handy)
- **Kanban**-Ansicht mit Lanes pro Konto (versteckte Kategorien `[K_<Lane>]`, alte Clients bleiben
  kompatibel), Drag & Drop, Lesebereich unten oder rechts
- **Heute**: alles heute Fällige plus die erste Kanban-Lane
- **Fokuszeit** (1–4 h): automatischer Abruf pausiert, manueller Abruf bleibt möglich
- Triage per Tastatur: **E** archivieren, **S** Wiedervorlage, **1–9** Lane; schlichtes Konfetti
  bei erledigten Aufgaben

**Kalender** (Microsoft 365)
- Tag / Arbeitswoche / Woche / Monat, mehrere Kalender, Termine und Besprechungen mit Teilnehmern,
  Teams-Besprechung, Serien; Einladungen annehmen / mit Vorbehalt / ablehnen

Versteckte Kategorien, die die App verwendet: `[K_…]` Kanban-Lane, `[#monthly]` usw. Wiederholung,
`[F_…]` Nachfass-Verknüpfung. Sie werden in der Oberfläche nicht als Tags angezeigt.

## Schnellstart ohne Konto (Demo-Modus)

```bash
cargo run -p outlook-clone -- --demo          # Desktop-App
cargo run -p devserver -- --demo --port 8080  # oder im Browser: http://127.0.0.1:8080
```

Ein eingebauter Fake-Graph-Server liefert Beispieldaten; nichts verlässt den Rechner.

## Mit Microsoft 365 verwenden

Die App-Registrierung von Symbiosika (Client-ID und Mandant) ist fest eingebaut – Anmelden genügt.
Das ist unbedenklich: Desktop-Apps sind öffentliche Clients ohne Geheimnis, beide IDs sind
öffentliche Kennungen. Mit dem fest eingebauten Mandanten können sich Konten dieses Tenants
anmelden; unter Einstellungen › Konten › Erweitert lassen sich Client-ID und Mandant ändern.
Für eigene Builds: `OUTLOOK_CLASSIC_CLIENT_ID=<guid> OUTLOOK_CLASSIC_TENANT=<guid|organizations> cargo build`.

**Anforderungen an die App-Registrierung** (Entra ID › App-Registrierungen):
1. Plattform **„Mobile und Desktopanwendungen“**, Umleitungs-URI `http://localhost`
2. Authentifizierung: **„Öffentliche Clientflows zulassen“ → Ja** (für den Gerätecode-Login)
3. Delegierte Microsoft-Graph-Berechtigungen: `User.Read`, `Mail.ReadWrite`, `Mail.Send`,
   `Calendars.ReadWrite`, `Contacts.ReadWrite`, `MailboxSettings.ReadWrite`, `People.Read`,
   `offline_access`, `openid`, `profile` – danach **„Administratorzustimmung erteilen“**

Login: Authorization Code Flow mit PKCE und Loopback-Redirect. Das Refresh-Token liegt im
Schlüsselbund des Betriebssystems (Windows Anmeldeinformationsverwaltung, macOS Schlüsselbund,
Linux Secret Service), sonst in einer nur für den Benutzer lesbaren Datei. IMAP-Passwörter ebenso.

### Verbindung prüfen (ohne Oberfläche)

```bash
cargo run -p devserver --bin live-check              # nur lesend, Login per Gerätecode
cargo run -p devserver --bin live-check -- --write   # zusätzlich Schreibtests (räumt auf)
```

Nutzt dieselbe Service-Schicht wie die App und gibt nur Zähler aus, keine Mailinhalte.

## Bauen

Voraussetzungen: Rust ≥ 1.80, unter Linux zusätzlich
`libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev libssl-dev libdbus-1-dev`.

```bash
cargo build --release -p outlook-clone          # Binärdatei: target/release/outlook-clone
cargo install tauri-cli --version "^2"          # einmalig, für Installer
cd src-tauri && cargo tauri build               # MSI/NSIS (Windows), DMG (macOS), deb/AppImage (Linux)
```

Installer für Windows, macOS und Linux baut auch der GitHub-Workflow `release.yml`
(manuell auslösbar oder per Tag `v*`).

## Architektur

```
ui/                    Oberfläche (HTML/CSS/ES-Module, kein Build-Schritt), läuft im WebView
src-tauri/             Tauri-Shell: Fenster, Dialoge, Benachrichtigungen, ein Befehl `call`
crates/core/           mailcore: OAuth, Graph- und IMAP/SMTP-Anbindung, Sync, SQLite-Cache, Service-API
crates/graph-fake/     zustandsbehafteter Fake von Microsoft Graph + SMTP-Sink (Tests, Demo-Modus)
crates/devserver/      dieselbe Service-API über HTTP/SSE (Browser-Entwicklung, E2E-Tests, live-check)
docs/API.md            Vertrag zwischen Oberfläche und Backend
tests/e2e/             Playwright-Tests der Oberfläche gegen den Dev-Server
```

## Tests

```bash
cargo test --workspace     # Kern, Fake-Server, IMAP gegen echtes Dovecot (übersprungen, wenn nicht installiert)
cd tests/e2e && npm ci && npx playwright install chromium && npx playwright test   # Oberfläche Ende-zu-Ende
```

Die IMAP-Tests starten einen privaten Dovecot (`apt install dovecot-imapd`). Die E2E-Tests bauen und
starten den Dev-Server selbst, jedes Mal mit frischen Demodaten. Der Playwright-Testrunner braucht
Node.js; unter Bun allein läuft er nicht (er beendet sich ohne Ausgabe).

### Oberfläche starten (ohne Node/Bun)

Die Oberfläche in `ui/` hat keinen Build-Schritt, das Rust-Backend liefert sie aus. Weder Bun noch
npm sind nötig (siehe Schnellstart). Änderungen an `ui/` werden im Browser-Modus mit einem Neuladen
der Seite (F5) sichtbar.
