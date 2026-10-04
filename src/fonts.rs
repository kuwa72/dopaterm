use glyphon::cosmic_text::fontdb::Database;

pub fn monospace_families(database: &Database) -> Vec<String> {
    let mut families: Vec<String> = database
        .faces()
        .filter(|face| face.monospaced)
        .filter_map(|face| face.families.first())
        .map(|(name, _)| name.clone())
        .filter(|name| !name.trim().is_empty())
        .collect();
    families.sort_by_key(|name| (name.to_lowercase(), name.clone()));
    families.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    families
}

pub fn resolve_family(name: Option<&str>, families: &[String]) -> Option<String> {
    let name = name?;
    families
        .iter()
        .find(|family| family.eq_ignore_ascii_case(name))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glyphon::cosmic_text::fontdb::{FaceInfo, ID, Language, Source};
    use std::sync::Arc;

    fn face(name: &str, monospaced: bool) -> FaceInfo {
        FaceInfo {
            id: ID::dummy(),
            source: Source::Binary(Arc::new(Vec::<u8>::new())),
            index: 0,
            families: vec![(name.to_string(), Language::English_UnitedStates)],
            post_script_name: name.to_string(),
            style: Default::default(),
            weight: Default::default(),
            stretch: Default::default(),
            monospaced,
        }
    }

    #[test]
    fn system_catalog_filters_attributes_and_deduplicates_families() {
        let mut database = Database::new();
        database.push_face_info(face("Zebra Fixed", true));
        database.push_face_info(face("Alpha Code", true));
        database.push_face_info(face("Alpha Code", true));
        database.push_face_info(face("alpha code", true));
        database.push_face_info(face("Mono In Name Only", false));
        database.push_face_info(face("", true));
        assert_eq!(monospace_families(&database), ["Alpha Code", "Zebra Fixed"]);
    }

    #[test]
    fn aliases_do_not_duplicate_the_same_face() {
        let mut database = Database::new();
        let mut font = face("MS Gothic", true);
        font.families
            .push(("ＭＳ ゴシック".into(), Language::Japanese_Japan));
        database.push_face_info(font);
        assert_eq!(monospace_families(&database), ["MS Gothic"]);
    }

    #[test]
    fn installed_catalog_only_contains_available_monospaced_faces() {
        let font_system = glyphon::FontSystem::new();
        let database = font_system.db();
        let families = monospace_families(database);
        for family in &families {
            assert!(database.faces().any(|face| {
                face.monospaced
                    && face
                        .families
                        .first()
                        .is_some_and(|(name, _)| name == family)
            }));
        }
        eprintln!("system monospace families: {}", families.len());
    }

    #[test]
    fn missing_or_proportional_configuration_is_not_selected() {
        let families = vec!["Cascadia Code".to_string()];
        assert_eq!(
            resolve_family(Some("cascadia code"), &families),
            Some("Cascadia Code".into())
        );
        assert_eq!(resolve_family(Some("Arial"), &families), None);
        assert_eq!(resolve_family(None, &families), None);
        assert_eq!(resolve_family(Some("Cascadia Code"), &[]), None);
    }
}
