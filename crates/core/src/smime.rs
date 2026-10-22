//! S/MIME: opaque-signed mails arrive as one `smime.p7m` attachment
//! (PKCS#7 / CMS SignedData) that wraps the real MIME message. This unwraps
//! the signed content so body and attachments can be shown. The signature
//! itself is not verified; encrypted mails (EnvelopedData) stay as they are.

/// OID 1.2.840.113549.1.7.2 (signedData), DER-encoded content bytes.
const OID_SIGNED_DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];
/// OID 1.2.840.113549.1.7.3 (envelopedData).
const OID_ENVELOPED_DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x03];

/// Whether an attachment is an S/MIME container (signed or encrypted).
pub fn is_p7m(name: &str, content_type: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let ct = content_type.to_ascii_lowercase();
    n.ends_with(".p7m") || ct.contains("pkcs7-mime")
}

/// Whether the container is encrypted (cannot be shown without the key).
pub fn is_encrypted(der: &[u8]) -> bool {
    content_info(der).is_some_and(|(oid, _)| oid == OID_ENVELOPED_DATA)
}

/// The MIME message inside an opaque-signed container, if it is one.
pub fn signed_content(der: &[u8]) -> Option<Vec<u8>> {
    let der = maybe_base64(der);
    let (oid, content) = content_info(&der)?;
    if oid != OID_SIGNED_DATA {
        return None;
    }
    // [0] EXPLICIT SignedData
    let (tag, signed, _) = read_tlv(content)?;
    if tag != 0xa0 {
        return None;
    }
    let (tag, sd, _) = read_tlv(signed)?;
    if tag != 0x30 {
        return None;
    }
    // SignedData: version, digestAlgorithms, encapContentInfo, …
    let (_, _, rest) = read_tlv(sd)?; // version
    let (_, _, rest) = read_tlv(rest)?; // digestAlgorithms
    let (tag, encap, _) = read_tlv(rest)?;
    if tag != 0x30 {
        return None;
    }
    let (_, _, rest) = read_tlv(encap)?; // eContentType
    let (tag, explicit, _) = read_tlv(rest)?;
    if tag != 0xa0 {
        return None; // detached signature: no content
    }
    let (tag, octets, _) = read_tlv(explicit)?;
    let mut out = Vec::new();
    collect_octets(tag, octets, &mut out)?;
    Some(out)
}

/// Some senders put the container base64-encoded into the part.
fn maybe_base64(raw: &[u8]) -> Vec<u8> {
    if raw.first() == Some(&0x30) {
        return raw.to_vec();
    }
    use base64::Engine;
    let clean: Vec<u8> = raw.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(&clean)
        .unwrap_or_else(|_| raw.to_vec())
}

/// ContentInfo ::= SEQUENCE { contentType OID, content [0] EXPLICIT ANY }
/// → (OID bytes, rest after the OID).
fn content_info(der: &[u8]) -> Option<(&[u8], &[u8])> {
    let (tag, seq, _) = read_tlv(der)?;
    if tag != 0x30 {
        return None;
    }
    let (tag, oid, rest) = read_tlv(seq)?;
    if tag != 0x06 {
        return None;
    }
    Some((oid, rest))
}

/// OCTET STRING content; BER allows a constructed string made of chunks.
fn collect_octets(tag: u8, data: &[u8], out: &mut Vec<u8>) -> Option<()> {
    match tag {
        0x04 => out.extend_from_slice(data),
        0x24 => {
            let mut rest = data;
            while !rest.is_empty() {
                let (t, chunk, r) = read_tlv(rest)?;
                collect_octets(t, chunk, out)?;
                rest = r;
            }
        }
        _ => return None,
    }
    Some(())
}

/// One BER element: (tag, content, rest). Indefinite lengths (0x80) are
/// resolved by walking the nested elements up to the end-of-contents marker.
fn read_tlv(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *data.first()?;
    if tag & 0x1f == 0x1f {
        return None; // multi-byte tags do not occur in CMS
    }
    let first = *data.get(1)?;
    let mut pos = 2;
    if first == 0x80 {
        if tag & 0x20 == 0 {
            return None; // only constructed elements may be indefinite
        }
        let body = &data[pos..];
        let mut rest = body;
        loop {
            if rest.len() >= 2 && rest[0] == 0 && rest[1] == 0 {
                let len = body.len() - rest.len();
                return Some((tag, &body[..len], &rest[2..]));
            }
            let (_, _, r) = read_tlv(rest)?;
            rest = r;
        }
    }
    let len = if first & 0x80 == 0 {
        first as usize
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 {
            return None;
        }
        let mut l = 0usize;
        for i in 0..n {
            l = (l << 8) | *data.get(pos + i)? as usize;
        }
        pos += n;
        l
    };
    let end = pos.checked_add(len)?;
    if end > data.len() {
        return None;
    }
    Some((tag, &data[pos..end], &data[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut v = vec![tag];
        if content.len() < 128 {
            v.push(content.len() as u8);
        } else {
            v.push(0x82);
            v.push((content.len() >> 8) as u8);
            v.push(content.len() as u8);
        }
        v.extend_from_slice(content);
        v
    }

    fn signed(inner: &[u8], indefinite: bool) -> Vec<u8> {
        let octets = if indefinite {
            // constructed OCTET STRING in two chunks, indefinite length
            let (a, b) = inner.split_at(inner.len() / 2);
            let mut v = vec![0x24, 0x80];
            v.extend(tlv(0x04, a));
            v.extend(tlv(0x04, b));
            v.extend([0, 0]);
            v
        } else {
            tlv(0x04, inner)
        };
        let encap = [tlv(0x06, &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x01]), tlv(0xa0, &octets)].concat();
        let sd = [tlv(0x02, &[1]), tlv(0x31, &[]), tlv(0x30, &encap), tlv(0x31, &[])].concat();
        tlv(0x30, &[tlv(0x06, OID_SIGNED_DATA), tlv(0xa0, &tlv(0x30, &sd))].concat())
    }

    #[test]
    fn unwraps_signed_content() {
        let inner = b"Content-Type: text/plain\r\n\r\nHallo zusammen";
        assert_eq!(signed_content(&signed(inner, false)).as_deref(), Some(&inner[..]));
        assert_eq!(signed_content(&signed(inner, true)).as_deref(), Some(&inner[..]));
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(signed(inner, false));
        assert_eq!(signed_content(b64.as_bytes()).as_deref(), Some(&inner[..]));
    }

    #[test]
    fn mail_with_p7m_shows_inner_body_and_attachments() {
        use base64::Engine;
        let inner = "Content-Type: multipart/mixed; boundary=\"in\"\r\n\r\n--in\r\nContent-Type: text/html\r\n\r\n<p>Hallo zusammen</p>\r\n--in\r\nContent-Type: application/vnd.openxmlformats-officedocument.wordprocessingml.document\r\nContent-Disposition: attachment; filename=\"Buehnendialog.docx\"\r\nContent-Transfer-Encoding: base64\r\n\r\nUEsDBA==\r\n--in--\r\n";
        let p7m = base64::engine::general_purpose::STANDARD.encode(signed(inner.as_bytes(), true));
        let raw = format!("From: a@example.com\r\nSubject: Test\r\nMIME-Version: 1.0\r\nContent-Type: application/pkcs7-mime; smime-type=signed-data; name=\"smime.p7m\"\r\nContent-Disposition: attachment; filename=\"smime.p7m\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{p7m}\r\n");
        let d = crate::imap::mime::detail(raw.as_bytes(), crate::imap::mime::summary("mb", "f", "1".into(), raw.as_bytes(), None, &[], None)).unwrap();
        assert!(d.signed);
        assert!(d.body_html.contains("Hallo zusammen"), "{}", d.body_html);
        let names: Vec<_> = d.attachments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Buehnendialog.docx"]);
        let (name, _, bytes) = crate::imap::mime::attachment_by_id(raw.as_bytes(), &d.attachments[0].id).unwrap();
        assert_eq!(name, "Buehnendialog.docx");
        assert_eq!(bytes, b"PK\x03\x04");
    }

    #[test]
    fn rejects_garbage_and_encrypted() {
        assert!(signed_content(b"").is_none());
        assert!(signed_content(&[0x30, 0x80, 0x06]).is_none());
        let env = tlv(0x30, &[tlv(0x06, OID_ENVELOPED_DATA), tlv(0xa0, &tlv(0x30, &[]))].concat());
        assert!(signed_content(&env).is_none());
        assert!(is_encrypted(&env));
        assert!(is_p7m("smime.p7m", "application/octet-stream"));
        assert!(is_p7m("x", "application/pkcs7-mime; smime-type=signed-data"));
    }
}
