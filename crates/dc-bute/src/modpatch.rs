//! MOD-21: mod content patches of the weapon table. The document and its types live in `dc_attr::modpatch`
//! (re-exported here); this module applies them to the ROM weapon table.

pub use dc_attr::modpatch::*;

#[cfg(test)]
use crate::ButeDoc;
use crate::WeaponsTable;
#[cfg(test)]
use std::collections::BTreeMap;

impl WeaponsTable {
    /// MOD-21: applies a mod set's weapon content to this table ([`WeaponContent::apply_to_doc`]).
    pub fn apply_content(&mut self, content: &WeaponContent) -> Result<(), PatchError> {
        content.apply_to_doc(&mut self.doc)
    }
}

#[cfg(test)]
mod variant_tests {
    use super::*;

    #[test]
    fn a_borrowed_look_brings_the_models_and_never_how_the_gun_plays() {
        let mut doc = ButeDoc::parse(concat!(
            "[Weapon27]\nName = \"L\"\nHHModel = \"a.ltb\"\nHHSkin0 = \"a.dtx\"\nPVSkin5 = \"only_a.dtx\"\n",
            "FireSndRadius = 3000\nFireSnd = \"loud.wav\"\nAmmoName0 = \"A\"\nPos = <1,1,1>\n",
            "[Weapon54]\nName = \"G\"\nHHModel = \"g.ltb\"\nHHSkin0 = \"g.dtx\"\nPVModelHand_St = \"g_pv.ltb\"\n",
            "FireSndRadius = 800\nFireSnd = \"quiet.wav\"\nAmmoName0 = \"G\"\nPos = <0,0,0>\n",
        ));
        let content = variant(504, &[(KEY_VARIANT_OF, "27"), (KEY_VARIANT_LOOK_OF, "54")]);
        content.apply_to_doc(&mut doc).unwrap();
        let sec = doc.get("Weapon504").unwrap();
        let get = |k: &str| sec.get(k).map(|v| v.trim_matches('"').to_string());
        assert_eq!(get("HHModel").as_deref(), Some("g.ltb"), "the look's model");
        assert_eq!(get("PVModelHand_St").as_deref(), Some("g_pv.ltb"));
        assert_eq!(get("Pos").as_deref(), Some("<0,0,0>"), "where it sits");
        assert_eq!(
            get("PVSkin5"),
            None,
            "none of the base gun's own look stays"
        );
        assert_eq!(
            get("FireSndRadius").as_deref(),
            Some("3000"),
            "heard as far as the base gun"
        );
        assert_eq!(get("FireSnd").as_deref(), Some("loud.wav"));
        assert_eq!(get("AmmoName0").as_deref(), Some("A"));
        assert_eq!(get("Name").as_deref(), Some("L"));

        let mut doc = ButeDoc::parse("[Weapon27]\nName = \"L\"\n");
        let missing = variant(504, &[(KEY_VARIANT_OF, "27"), (KEY_VARIANT_LOOK_OF, "54")]);
        assert!(missing.apply_to_doc(&mut doc).is_err());
    }

    fn doc() -> ButeDoc {
        ButeDoc::parse(concat!(
            "[Weapon25]\nName = \"G\"\n",
            "ShotsPerClip = 30\n",
            "InstDamage = 20\nPVSkin1 = \"a.dtx\"\nHHSkin0 = \"b.dtx\"\n",
        ))
    }

    fn variant(row: u16, fields: &[(&str, &str)]) -> WeaponContent {
        WeaponContent::new(
            vec![WeaponPatch {
                weapon: row,
                op: PatchOp::Register,
                fields: fields
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            }],
            Vec::new(),
        )
        .unwrap()
    }

    /// CLI-1845: a gun's weapon inspect is every one of its variants' too, whatever order the packs come in, and a variant
    /// cannot set one of its own (the inspect keys are not cosmetic keys).
    #[test]
    fn a_guns_inspect_is_its_variants_inspect() {
        let patch = |weapon: u16, op: PatchOp, fields: &[(&str, &str)]| WeaponPatch {
            weapon,
            op,
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let inspect = [
            ("InspectLen", "3900"),
            ("InspectPose1", "480 16 -50 -3 -2.6 0.4 2.9"),
        ];
        let camo = [("variant_of", "25"), ("PVSkin1", "@mod:p/assets/x.dtx")];
        let c = WeaponContent::new(
            vec![
                patch(500, PatchOp::Register, &camo),
                patch(25, PatchOp::Patch, &inspect),
            ],
            Vec::new(),
        )
        .unwrap();
        let mut d = doc();
        c.apply_to_doc(&mut d).unwrap();
        for (k, v) in inspect {
            assert_eq!(
                d.sections["Weapon500"].get(k).map(String::as_str),
                Some(v),
                "{k}"
            );
        }
        assert!(!is_cosmetic_key("InspectLen") && !is_cosmetic_key("InspectPose1"));
        let own = [("variant_of", "25"), ("InspectLen", "100")];
        let refused = WeaponContent::new(vec![patch(501, PatchOp::Register, &own)], Vec::new())
            .and_then(|c| c.apply_to_doc(&mut doc()));
        assert!(refused.is_err(), "a variant may not set its own inspect");
    }

    /// VAR-2: a variant has its base row's every gameplay key, and differs only where it is allowed to.
    #[test]
    fn a_variant_keeps_every_gameplay_key_of_its_base() {
        let mut d = doc();
        let c = variant(
            500,
            &[("variant_of", "25"), ("PVSkin1", "@mod:p/assets/x.dtx")],
        );
        c.apply_to_doc(&mut d).unwrap();
        let (base, row) = (&d.sections["Weapon25"], &d.sections["Weapon500"]);
        for (k, v) in base.iter() {
            if k != "PVSkin1" {
                assert_eq!(row.get(k), Some(v), "{k}");
            }
        }
        assert_eq!(
            row.get("PVSkin1").map(String::as_str),
            Some("@mod:p/assets/x.dtx")
        );
        assert_eq!(
            base.get("PVSkin1").map(String::as_str),
            Some("a.dtx"),
            "the base is untouched"
        );
        let v = c.variants();
        assert_eq!((v[0].row, v[0].base), (500, 25));
    }

    /// CRATE-6: a variant names its rarity tier; one without the key is Common, an unknown tier is refused.
    #[test]
    fn a_variant_names_its_rarity_and_an_unknown_one_is_refused() {
        let mut d = doc();
        let c = variant(
            500,
            &[("variant_of", "25"), ("variant_rarity", "Legendary")],
        );
        c.apply_to_doc(&mut d).unwrap();
        assert_eq!(c.variants()[0].rarity, Rarity::Legendary);
        let plain = variant(501, &[("variant_of", "25")]);
        assert_eq!(plain.variants()[0].rarity, Rarity::Common);
        let e = WeaponContent::new(
            vec![WeaponPatch {
                weapon: 502,
                op: PatchOp::Register,
                fields: [("variant_of", "25"), ("variant_rarity", "mythic")]
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            }],
            Vec::new(),
        )
        .unwrap_err();
        assert!(e.0.contains("mythic"), "{e}");
        assert!(Rarity::ALL
            .iter()
            .all(|r| Rarity::parse(r.key()) == Some(*r) && Rarity::parse(r.name()) == Some(*r)));
        assert!(Rarity::Common < Rarity::Legendary);
    }

    #[test]
    fn a_variant_may_not_change_gameplay_or_stack() {
        let mut d = doc();
        let before = d.clone();
        let e = variant(500, &[("variant_of", "25"), ("InstDamage", "99")])
            .apply_to_doc(&mut d)
            .unwrap_err();
        assert!(e.0.contains("InstDamage"), "{e}");
        assert!(variant(500, &[("variant_of", "7")])
            .apply_to_doc(&mut d)
            .is_err());
        assert_eq!(d.sections, before.sections, "a refusal changes nothing");
        variant(500, &[("variant_of", "25")])
            .apply_to_doc(&mut d)
            .unwrap();
        assert!(variant(501, &[("variant_of", "500")])
            .apply_to_doc(&mut d)
            .is_err());
    }

    /// A base patched by the same document: the variant follows it, so the two still play the same.
    #[test]
    fn a_variant_follows_a_patch_of_its_base() {
        let mut d = doc();
        let c = WeaponContent::new(
            vec![
                WeaponPatch {
                    weapon: 25,
                    op: PatchOp::Patch,
                    fields: [("InstDamage".to_string(), "21".to_string())].into(),
                },
                WeaponPatch {
                    weapon: 500,
                    op: PatchOp::Register,
                    fields: [("variant_of".to_string(), "25".to_string())].into(),
                },
            ],
            Vec::new(),
        )
        .unwrap();
        c.apply_to_doc(&mut d).unwrap();
        assert_eq!(
            d.sections["Weapon500"]
                .get("InstDamage")
                .map(String::as_str),
            Some("21")
        );
    }

    #[test]
    fn only_looks_are_cosmetic_keys() {
        for k in [
            "PVSkin1",
            "HHSkin0",
            "PVRenderStyle2",
            "HHModel",
            "PVModelHand_St",
            "variant_name",
        ] {
            assert!(is_cosmetic_key(k), "{k}");
        }
        for k in [
            "InstDamage",
            "ShotsPerClip",
            "PVSkin",
            "PVSkinX",
            "ModName0",
            "Name",
            "AmmoName",
        ] {
            assert!(!is_cosmetic_key(k), "{k}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // made-up rows in the key shape of the ROM weapon table
    const DOC: &str = concat!(
        "[Ammo0]\nName = \"Round A\"\nInstDamage = 25\n",
        "[Weapon0]\nName = \"Gun A\"\n",
        "ShotsPerClip = 30\nAmmoName0 = \"Round A\"\n",
        "[Weapon145]\nName = \"Gun B\"\n",
        "ShotsPerClip = 10\n",
    );

    fn fields(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn content() -> WeaponContent {
        WeaponContent::new(
            vec![
                WeaponPatch {
                    weapon: 145,
                    op: PatchOp::Patch,
                    fields: fields(&[("ShotsPerClip", "12"), ("AmmoDamage", "41")]),
                },
                WeaponPatch {
                    weapon: 300,
                    op: PatchOp::Register,
                    fields: fields(&[("Name", "\"Gun C\""), ("ShotsPerClip", "5")]),
                },
            ],
            vec![AmmoPatch {
                ammo: 0,
                op: PatchOp::Patch,
                fields: fields(&[("InstDamage", "30.5")]),
            }],
        )
        .unwrap()
    }

    #[test]
    fn patch_overwrites_keys_and_keeps_the_rest() {
        let mut doc = ButeDoc::parse(DOC);
        content().apply_to_doc(&mut doc).unwrap();
        assert_eq!(doc.get_str("Weapon145", "ShotsPerClip"), Some("12"));
        assert_eq!(
            doc.get_str("Weapon145", "AmmoDamage"),
            Some("41"),
            "unknown keys are allowed"
        );
        assert_eq!(doc.get_str("Weapon145", "Name"), Some("Gun B"));
        assert_eq!(doc.get_str("Ammo0", "InstDamage"), Some("30.5"));
        assert_eq!(doc.get_str("Weapon0", "ShotsPerClip"), Some("30"));
    }

    #[test]
    fn register_adds_a_new_row_in_order() {
        let mut t = WeaponsTable::from_doc(ButeDoc::parse(DOC));
        t.apply_content(&content()).unwrap();
        let row = t.weapon(300).expect("registered row");
        assert_eq!(
            row.get("Name").map(String::as_str),
            Some("Gun C"),
            "quotes stripped"
        );
        assert_eq!(row.len(), 2);
        assert_eq!(t.doc().order.last().map(String::as_str), Some("Weapon300"));
    }

    #[test]
    fn a_bad_id_refuses_the_whole_document() {
        let mut doc = ButeDoc::parse(DOC);
        let missing = WeaponContent::new(
            vec![
                WeaponPatch {
                    weapon: 145,
                    op: PatchOp::Patch,
                    fields: fields(&[("ShotsPerClip", "99")]),
                },
                WeaponPatch {
                    weapon: 7,
                    op: PatchOp::Patch,
                    fields: fields(&[("ShotsPerClip", "1")]),
                },
            ],
            vec![],
        )
        .unwrap();
        let e = missing.apply_to_doc(&mut doc).unwrap_err();
        assert!(e.0.contains("[Weapon7]"), "{e}");
        assert_eq!(
            doc.get_str("Weapon145", "ShotsPerClip"),
            Some("10"),
            "nothing applied"
        );
        let taken = WeaponContent::new(
            vec![WeaponPatch {
                weapon: 0,
                op: PatchOp::Register,
                fields: fields(&[("Name", "X")]),
            }],
            vec![],
        )
        .unwrap();
        assert!(taken
            .apply_to_doc(&mut doc)
            .unwrap_err()
            .0
            .contains("already exists"));
        // duplicate ids, a bad key, a bad value
        let dup = |op| WeaponPatch {
            weapon: 1,
            op,
            fields: BTreeMap::new(),
        };
        assert!(
            WeaponContent::new(vec![dup(PatchOp::Patch), dup(PatchOp::Register)], vec![]).is_err()
        );
        let bad_key = WeaponPatch {
            weapon: 1,
            op: PatchOp::Patch,
            fields: fields(&[("Shots PerClip", "1")]),
        };
        assert!(WeaponContent::new(vec![bad_key], vec![]).is_err());
        let bad_value = WeaponPatch {
            weapon: 1,
            op: PatchOp::Patch,
            fields: fields(&[("Name", "a\nb")]),
        };
        assert!(WeaponContent::new(vec![bad_value], vec![]).is_err());
    }

    #[test]
    fn json_round_trips_in_canonical_form_and_the_hash_is_stable() {
        let c = content();
        let j = c.to_json();
        assert_eq!(
            j,
            concat!(
                "{\"version\":1,\"weapons\":[",
                "{\"weapon\":145,\"op\":\"patch\",\"fields\":{\"AmmoDamage\":\"41\",\"ShotsPerClip\":\"12\"}},",
                "{\"weapon\":300,\"op\":\"register\",\"fields\":{\"Name\":\"Gun C\",\"ShotsPerClip\":\"5\"}}],",
                "\"ammo\":[{\"ammo\":0,\"op\":\"patch\",\"fields\":{\"InstDamage\":\"30.5\"}}]}"
            )
        );
        let back = WeaponContent::from_json(&j).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.hash(), c.hash());
        assert_eq!(c.hash().len(), 64);
        // spacing and row order of the file do not change the hash
        let spaced = r#"{ "ammo": [ {"fields": {"InstDamage": "30.5"}, "op": "patch", "ammo": 0} ],
            "weapons": [ {"weapon": 300, "op": "register", "fields": {"ShotsPerClip": "5", "Name": "Gun C"}},
                         {"weapon": 145, "op": "patch", "fields": {"ShotsPerClip": "12", "AmmoDamage": "41"}} ],
            "version": 1 }"#;
        assert_eq!(WeaponContent::from_json(spaced).unwrap().hash(), c.hash());
        // a different value is a different hash
        let mut other = c.clone();
        other.ammo[0]
            .fields
            .insert("InstDamage".into(), "31".into());
        assert_ne!(other.hash(), c.hash());
        assert!(WeaponContent::default().is_empty());
    }

    #[test]
    fn malformed_documents_are_refused() {
        for bad in [
            "",
            "[]",
            "{\"version\":2,\"weapons\":[]}",
            "{\"weapons\":[]}",
            "{\"version\":1,\"weapons\":{}}",
            "{\"version\":1,\"weapons\":[{\"weapon\":70000,\"op\":\"patch\",\"fields\":{}}]}",
            "{\"version\":1,\"weapons\":[{\"weapon\":1,\"op\":\"replace\",\"fields\":{}}]}",
            "{\"version\":1,\"weapons\":[{\"weapon\":1,\"op\":\"patch\",\"fields\":{\"A\":41}}]}",
            "{\"version\":1,\"weapons\":[{\"weapon\":1,\"op\":\"patch\",\"fields\":{},\"x\":1}]}",
            "{\"version\":1,\"extra\":[]}",
        ] {
            assert!(WeaponContent::from_json(bad).is_err(), "{bad}");
        }
        assert!(WeaponContent::from_json("{\"version\":1}")
            .unwrap()
            .is_empty());
    }
}
