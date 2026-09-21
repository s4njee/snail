use snail_core::pgp_mime::{
    CryptoEnvelope, canonicalize_signed_entity, envelope, multipart_signed,
};

#[test]
fn gnupg_and_thunderbird_rfc3156_shapes_round_trip() {
    for (name, fixture) in [
        (
            "GnuPG",
            include_bytes!("fixtures/gnupg-multipart-signed.eml").as_slice(),
        ),
        (
            "Thunderbird",
            include_bytes!("fixtures/thunderbird-multipart-signed.eml").as_slice(),
        ),
    ] {
        let CryptoEnvelope::DetachedSigned { entity, signature } = envelope(fixture) else {
            panic!("{name} fixture was not recognized");
        };
        let canonical = canonicalize_signed_entity(&entity);
        assert!(canonical.ends_with(b"\r\n"), "{name}");

        // Their output -> Snail parser, followed by Snail output -> the same parser. The signature
        // bytes are intentionally a structural fixture; cryptographic fixtures exercise pgp.rs.
        let rebuilt = multipart_signed(&canonical, &signature, "snail-interop").unwrap();
        let CryptoEnvelope::DetachedSigned {
            entity: rebuilt_entity,
            signature: rebuilt_sig,
        } = envelope(&rebuilt)
        else {
            panic!("Snail could not parse its rebuilt {name} message");
        };
        assert_eq!(canonicalize_signed_entity(&rebuilt_entity), canonical);
        let normalize = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace("\r\n", "\n");
        assert_eq!(normalize(&rebuilt_sig), normalize(&signature));
    }
}
