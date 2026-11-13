#![no_main]
use libfuzzer_sys::fuzz_target;
use octofhir_hl7v2::{
    schema::{
        CompileLimits, MatchLimits,
        fields::ValidationLimits,
        package::{LoadLimits, OwnedPackage},
    },
    wire::Message,
};
const PIN: [u8; 32] = [
    0x01, 0x61, 0x08, 0x1c, 0x3e, 0xb1, 0x55, 0x36, 0xd2, 0x63, 0xba, 0x1b, 0x1f, 0x35, 0x77, 0x25,
    0xb0, 0xef, 0xc4, 0x3b, 0x7e, 0x2d, 0x02, 0xa5, 0x8b, 0x54, 0x1d, 0xda, 0x1e, 0x66, 0x9c, 0xd1,
];
const CODE_PIN: [u8; 32] = [
    0x95, 0x99, 0x42, 0xd4, 0x03, 0x06, 0x16, 0x1c, 0xf3, 0x28, 0xd5, 0x07, 0x7b, 0x24, 0x2b, 0x94,
    0x66, 0x24, 0x45, 0xb0, 0x57, 0xb1, 0xf6, 0xfb, 0x25, 0x3b, 0x91, 0xb5, 0x6e, 0xb6, 0x82, 0xef,
];
const CONDITION_PIN: [u8; 32] = [
    0x47, 0xcc, 0xa6, 0x23, 0x56, 0x0e, 0x9e, 0x6c, 0x4c, 0xf4, 0x7e, 0x3e, 0x0b, 0x56, 0xfd, 0x60,
    0x53, 0xbc, 0xa1, 0xf9, 0xa5, 0x87, 0x10, 0x10, 0xa5, 0x24, 0x4d, 0x46, 0x6a, 0x04, 0x74, 0x69,
];
fuzz_target!(|bytes: &[u8]| {
    let limits = LoadLimits {
        max_bytes: 16_384,
        max_items: 256,
        max_text_bytes: 4096,
        compile: CompileLimits {
            max_rules: 128,
            max_depth: 8,
            max_work: 4096,
        },
    };
    if bytes.len() > limits.max_bytes {
        return;
    }
    for (pin, version, canonical, source) in [
        (
            PIN,
            1,
            include_bytes!("../../tests/fixtures/native-package-v1.bin").as_slice(),
            include_bytes!("../../tests/fixtures/native-package.er7").as_slice(),
        ),
        (
            CODE_PIN,
            2,
            include_bytes!("../../tests/fixtures/native-package-v2.bin").as_slice(),
            include_bytes!("../../tests/fixtures/native-codes.er7").as_slice(),
        ),
        (
            CONDITION_PIN,
            3,
            include_bytes!("../../tests/fixtures/native-package-v3.bin").as_slice(),
            include_bytes!("../../tests/fixtures/native-conditions.er7").as_slice(),
        ),
    ] {
        if let Ok(loaded) = OwnedPackage::decode(bytes, pin, limits) {
            assert_eq!(loaded.digest(), pin);
            assert_eq!(loaded.format_version(), version);
            let encoded = loaded.encode(limits.max_bytes);
            assert!(encoded.is_ok());
            if let Ok(encoded) = encoded {
                assert!(OwnedPackage::read_from(encoded.as_slice(), pin, limits).is_ok());
                // Nonsemantic table/code ordering converges to the independently encoded export.
                assert_eq!(encoded, canonical);
            }
            let message = Message::parse(source);
            assert!(message.is_ok());
            if let Ok(message) = message {
                let view = loaded.catalog().group(&message, MatchLimits::default());
                assert!(view.is_ok());
                if let Ok(view) = view {
                    assert!(
                        loaded
                            .fields()
                            .validate(&view, ValidationLimits::default())
                            .is_valid()
                    );
                    assert_eq!(
                        view.message().original().as_ptr(),
                        message.original().as_ptr()
                    );
                }
            }
        }
    }
});
