# Outlook Classic – E-Mail & Kalender für Microsoft 365

Ein Desktop-Mailprogramm in **Rust** (Tauri 2), das sich bewusst am **klassischen Outlook / eM Client**
orientiert: Ordnerbaum links, Nachrichtenliste mit Spalten in der Mitte, Lesebereich rechts,
Navigationsschaltflächen (E-Mail / Kalender) unten links, Statusleiste – dicht und übersichtlich.

Es spricht ausschließlich die **Microsoft Graph API** (EWS wird von Microsoft abgeschaltet).

## Funktionen

**E-Mail**
- Mehrere Konten, zusätzlich **freigegebene Postfächer** (`/users/{adresse}`)
- Vollständiger Ordnerbaum inkl. Unterordner, Favoriten, Ungelesen-Zähler
- Ordner anlegen / umbenennen / löschen, „Alle als gelesen markieren“, „Ordner leeren“
- Spaltenliste (Wichtigkeit, Anhang, Von, Betreff, Erhalten, Kategorien, Kennzeichnung), sortierbar,
  Spaltenbreiten veränderbar, Gruppierung nach Datum (Heute, Gestern, Letzte Woche …), optional
  zweizeilige eM-Client-Ansicht
- Filter: Alle / Ungelesen / Gekennzeichnet / Kategorie, Schnellfilter lokal, Serversuche (Enter)
- Lesebereich rechts, unten oder aus; Nachricht im eigenen Fenster per Doppelklick
- HTML-Mails in einem **abgeschotteten WebView-iframe** (keine Skripte, CSP); externe Bilder werden
  standardmäßig blockiert („Bilder herunterladen“), Inline-Bilder (`cid:`) werden aufgelöst
- Antworten, Allen antworten, Weiterleiten (serverseitige Entwürfe → korrekte Konversationen),
  Signatur, Rich-Text-Editor, Cc/Bcc mit Adressvorschlägen (People-API + bekannte Absender),
  Wichtigkeit, Lesebestätigung, Anhänge (große Anhänge > 3 MB per Upload-Session), Entwürfe
- Verschieben/Kopieren (auch Drag & Drop), Löschen (in „Gelöschte Elemente“ bzw. endgültig),
  Gelesen/Ungelesen, Kennzeichnung (Kennzeichnen / Erledigt), Quelltext anzeigen
- **Kategorien**: Outlook-Masterkategorien mit allen 25 Farben – zuweisen, anlegen, Farbe ändern,
  löschen; farbige Anzeige in Liste, Lesebereich und Kalender
- Abwesenheitsnotiz (intern/extern, zeitgesteuert)
- Hintergrund-Synchronisation per **Delta-Abfragen**, Desktop-Benachrichtigung bei neuer Mail,
  lokaler **SQLite-Cache** (schnelles Sortieren, Offline-Lesen)
- Tastenkürzel wie in Outlook: Strg+N, Strg+R, Strg+Umschalt+R, Strg+F, Strg+Q/U, Entf, F9 …

**Kalender**
- Ansichten Tag / Arbeitswoche / Woche / Monat, Datumsnavigator, mehrere Kalender mit Farben
- Termine und Besprechungen anlegen/bearbeiten/löschen, Teilnehmer, Teams-Besprechung,
  Erinnerung, „Anzeigen als“, Kategorien, Vertraulichkeit, Serien (täglich/wöchentlich/monatlich/jährlich)
- Einladungen annehmen / mit Vorbehalt / ablehnen – auch direkt aus der Einladungs-Mail

## Schnellstart ohne Microsoft-Konto (Demo-Modus)

```bash
cargo run -p outlook-clone -- --demo
```

Der Demo-Modus startet einen eingebauten Fake-Graph-Server mit Beispieldaten (Mails, Ordner,
Kategorien, Kalender) und meldet sich automatisch an. Alles ist bedienbar, nichts verlässt den Rechner.

Alternativ im Browser (Entwicklung): `cargo run -p devserver -- --demo --port 8080` → http://127.0.0.1:8080

## Mit Microsoft 365 verwenden

### 1. App in Entra ID (Azure AD) registrieren – einmalig, ca. 3 Minuten

1. https://entra.microsoft.com → **Anwendungen › App-Registrierungen › Neue Registrierung**
2. Name z. B. „Outlook Classic“, *Unterstützte Kontotypen*: „Nur Konten in diesem Organisationsverzeichnis“
   (oder mehrinstanzenfähig, wenn mehrere Tenants genutzt werden sollen)
3. **Umleitungs-URI**: Plattform **„Öffentlicher Client/nativ (mobil und Desktop)“**, URI `http://localhost`
4. Nach dem Anlegen unter **Authentifizierung** „Öffentliche Clientflows zulassen“ → **Ja**
5. Unter **API-Berechtigungen › Microsoft Graph › Delegierte Berechtigungen** hinzufügen:
   `User.Read`, `Mail.ReadWrite`, `Mail.ReadWrite.Shared`, `Mail.Send`, `Mail.Send.Shared`,
   `Calendars.ReadWrite`, `Calendars.ReadWrite.Shared`, `MailboxSettings.ReadWrite`, `People.Read`,
   `offline_access`, `openid`, `profile` – danach **„Administratorzustimmung erteilen“**
6. Die **Anwendungs-ID (Client-ID)** kopieren (und ggf. die Verzeichnis-ID/Tenant-ID)

### 2. Anmelden

App starten → im Willkommensdialog Client-ID und Tenant (`organizations`, Tenant-ID oder Domain)
eintragen → **Anmelden**. Der Standardbrowser öffnet die Microsoft-Anmeldung (inkl. MFA);
danach kehrt man automatisch zur App zurück (Authorization Code Flow mit PKCE, Loopback-Redirect).

Das Refresh-Token liegt im Schlüsselbund des Betriebssystems (Windows Anmeldeinformationsverwaltung,
macOS Schlüsselbund, Linux Secret Service); ist keiner verfügbar, in einer nur für den Benutzer
lesbaren Datei im App-Datenverzeichnis.

### Verbindung prüfen (ohne GUI)

```bash
cargo run -p devserver --bin live-check -- --client-id <GUID> --tenant <Tenant-ID>          # nur lesend
cargo run -p devserver --bin live-check -- --client-id <GUID> --tenant <Tenant-ID> --write  # + Schreibtests
```

Anmeldung per Gerätecode (microsoft.com/devicelogin) – dafür muss in der App-Registrierung
„Öffentliche Clientflows zulassen“ aktiviert sein. Das Werkzeug nutzt dieselbe Service-Schicht wie die
App, gibt nur Zähler aus (keine Mailinhalte) und räumt bei `--write` alle Testobjekte wieder auf.

## Bauen

Voraussetzungen: Rust ≥ 1.80, unter Linux zusätzlich
`libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev libssl-dev`.

```bash
cargo build --release -p outlook-clone          # Binärdatei: target/release/outlook-clone
cargo install tauri-cli --version "^2"          # einmalig, für Installer
cd src-tauri && cargo tauri build               # MSI/NSIS (Windows), DMG (macOS), deb/AppImage (Linux)
```

Fertige Installer für Windows, macOS und Linux baut auch der GitHub-Workflow `release.yml`
(manuell auslösbar oder per Tag `v*`).

## Architektur

```
ui/                    Oberfläche (HTML/CSS/ES-Module, kein Build-Schritt), läuft im WebView
src-tauri/             Tauri-Shell: Fenster, Dialoge, Benachrichtigungen, ein Befehl `call`
crates/core/           mailcore: OAuth (PKCE), Graph-Client, Delta-Sync, SQLite-Cache, Service-API
crates/graph-fake/     zustandsbehafteter Fake von Microsoft Graph (Tests + Demo-Modus)
crates/devserver/      dieselbe Service-API über HTTP/SSE – für Browser-Entwicklung und E2E-Tests
docs/API.md            Vertrag zwischen Oberfläche und Backend
tests/e2e/             Playwright-Tests der Oberfläche gegen den Dev-Server
```

## Tests

```bash
cargo test --workspace                 # Kern + Fake-Server (inkl. Login, Delta, Senden, Kalender)
cd tests/e2e && npm ci && npx playwright test   # UI-Ende-zu-Ende (startet den Dev-Server selbst)
```
