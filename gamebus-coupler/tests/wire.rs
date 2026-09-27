//! Golden wire shapes. beisl reads and writes these; a rename or a retag
//! must fail here, not quietly on the other side.

use gamebus_coupler::{CompatFinding, FindingVerb, Miss, MissChange, MissVerb, Refusal, WallKind};
use serde_json::{json, Value};

fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(golden: Value) {
    let parsed: T = serde_json::from_value(golden.clone()).expect("golden parses");
    assert_eq!(serde_json::to_value(&parsed).unwrap(), golden);
}

#[test]
fn verbs() {
    let cases = [
        (MissVerb::Dismiss, json!({"verb": "dismiss"})),
        (MissVerb::Demote, json!({"verb": "demote"})),
        (
            MissVerb::SetTitle {
                title: "Control".into(),
            },
            json!({"verb": "set-title", "title": "Control"}),
        ),
        (
            MissVerb::SetIdentity {
                store: None,
                codename: "Calluna".into(),
            },
            json!({"verb": "set-identity", "codename": "Calluna"}),
        ),
        (
            MissVerb::PickEntry {
                store: "gog".into(),
                codename: "1".into(),
                umu_id: "umu-1".into(),
            },
            json!({"verb": "pick-entry", "store": "gog", "codename": "1", "umu_id": "umu-1"}),
        ),
    ];
    for (verb, golden) in cases {
        assert_eq!(serde_json::to_value(&verb).unwrap(), golden);
        assert_eq!(serde_json::from_value::<MissVerb>(golden).unwrap(), verb);
    }
    assert_eq!(
        serde_json::to_value(FindingVerb::MarkReported {
            target: "awacy".into()
        })
        .unwrap(),
        json!({"verb": "mark-reported", "target": "awacy"})
    );
    assert_eq!(
        serde_json::to_value(MissChange::StoreSet {
            guessed: "none".into()
        })
        .unwrap(),
        json!({"change": "store-set", "guessed": "none"})
    );
    assert_eq!(
        serde_json::to_value(Refusal::NotAUmuMiss).unwrap(),
        json!({"refusal": "not-a-umu-miss"})
    );
    assert_eq!(
        serde_json::to_value(Refusal::UnknownTarget { target: "x".into() }).unwrap(),
        json!({"refusal": "unknown-target", "target": "x"})
    );
}

#[test]
fn a_miss() {
    round_trip::<Miss>(json!({
        "title": "Severed Steel",
        "store": "gog",
        "codename": "1242122770",
        "umu_id": "umu-default",
        "title_source": "lutris-wrapper",
        "confidence": "medium",
        "executable": "/usr/bin/python3.13",
        "first_seen": "2026-09-23",
        "last_seen": "2026-09-23",
        "launcher": "lutris",
        "runner": "proton",
        "verification": {"state": "confirmed-missing", "umu_id": null, "checked": "2026-09-23"},
        "drafted_id": {"id": "umu-1227690", "basis": "steam-sku", "collision_checked": "2026-09-23"},
        "fix": {
            "umu_id": "umu-1227690",
            "fixes": [],
            "local": ["/home/u/.config/protonfixes/localfixes/umu-1227690.py"],
            "checked": "2026-09-23"
        },
        "codename_override": "1242122770",
        "codename_override_source": "lutris-library",
        "dismissed": "2026-09-24",
        "umu_promoted": "2026-09-24"
    }));
}

#[test]
fn a_finding_keeps_unknown_kinds_and_fields() {
    let golden = json!({
        "wall": "some-future-wall",
        "first_seen": "2026-09-05",
        "last_seen": "2026-09-06",
        "observations": [{
            "source": "beisl",
            "observed": "2026-09-06",
            "wine": "prater-11.0-3",
            "gpu_vendor": "amd",
            "signature": "elytra_*",
            "layer_split": {"wine": 12.5},
            "record_mode": "degraded",
            "a_newer_field": [1, 2]
        }],
        "reported": {"protondb": "2026-09-07"},
        "from_a_newer_beisl": true
    });
    round_trip::<CompatFinding>(golden.clone());
    let f: CompatFinding = serde_json::from_value(golden).unwrap();
    assert_eq!(f.wall, WallKind::Other("some-future-wall".into()));
}
