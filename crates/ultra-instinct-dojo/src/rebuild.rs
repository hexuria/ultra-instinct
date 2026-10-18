//! Rebuild replay inputs from diary lines: the recorded offered menu back
//! into an `ActionSpace` a policy can decide on again.
//!
//! The rebuilt space is exactly the recorded menu — controls and
//! target-bound actions alike — so a replayed decision sees what the live
//! decision saw. Diaries written before `region` was recorded keep working:
//! those actions rebuild as controls (a degraded but honest replay).

use ultra_instinct_core::{
    ActionId, ActionKind, ActionSpace, ElementState, ObservedAction, RegionId, Role,
};

use crate::line::{DecisionLine, OfferedState};

/// The `ActionSpace` a recorded decision was made against.
///
/// Malformed entries (unparseable action ids or kinds) are skipped rather
/// than failing the whole replay — a torn diary still reports on the
/// decisions it can rebuild.
pub fn action_space(line: &DecisionLine) -> ActionSpace {
    let actions = line
        .offered
        .iter()
        .filter_map(|offered| {
            let id = ActionId::try_new(&offered.id).ok()?;
            let kind = ActionKind::parse(&offered.kind)?;
            let target = offered
                .region
                .as_deref()
                .and_then(|raw| RegionId::try_new(raw).ok());
            let role = offered.role.as_deref().and_then(Role::parse);
            let state = offered
                .state
                .as_ref()
                .map(element_state)
                .unwrap_or_default();
            Some(ObservedAction::new(
                id,
                kind,
                target,
                offered.label.clone(),
                role,
                offered.fingerprint,
                state,
            ))
        })
        .collect();
    ActionSpace::from_actions(actions)
}

/// Control state recorded on an offered action → `ElementState`.
pub fn element_state(state: &OfferedState) -> ElementState {
    ElementState {
        value: state.value.clone(),
        checked: state.checked,
        expanded: state.expanded,
        selected: state.selected.clone(),
        options: state.options.clone(),
        input_type: state.input_type.clone(),
        href: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line::{DecisionLine, OfferedLine};

    fn decision(offered: Vec<OfferedLine>) -> DecisionLine {
        DecisionLine {
            seq: 1,
            clause_index: 0,
            clause: "go".to_owned(),
            mode: "act".to_owned(),
            source: "instinct".to_owned(),
            site: None,
            situation: Default::default(),
            offered,
            operation_ranked: vec![],
            target_ranked: vec![],
            history: vec![],
            choice: None,
            abstain: Some("no candidates".to_owned()),
        }
    }

    #[test]
    fn rebuilds_target_bound_and_control_actions() {
        let line = decision(vec![
            OfferedLine {
                id: "CLICK:go".to_owned(),
                kind: "CLICK".to_owned(),
                label: "Go".to_owned(),
                role: Some("button".to_owned()),
                region: Some("go".to_owned()),
                fingerprint: 42,
                state: None,
            },
            OfferedLine {
                id: "DONE".to_owned(),
                kind: "DONE".to_owned(),
                label: "done".to_owned(),
                role: None,
                region: None,
                fingerprint: 0,
                state: None,
            },
            // Torn line: unparseable kind is skipped, not fatal.
            OfferedLine {
                id: "BOGUS:x".to_owned(),
                kind: "BOGUS".to_owned(),
                label: "x".to_owned(),
                role: None,
                region: None,
                fingerprint: 0,
                state: None,
            },
        ]);
        let space = action_space(&line);
        assert_eq!(space.len(), 2);
        let click = space.get_str("CLICK:go").unwrap();
        assert_eq!(click.kind(), ActionKind::Click);
        assert_eq!(click.target().map(RegionId::as_str), Some("go"));
        assert_eq!(click.role(), Some(Role::Button));
        assert_eq!(click.target_fingerprint(), 42);
        assert!(space.get_str("DONE").unwrap().target().is_none());
        assert!(space.get_str("BOGUS:x").is_none());
    }

    #[test]
    fn missing_region_rebuilds_as_control() {
        let line = decision(vec![OfferedLine {
            id: "CLICK:go".to_owned(),
            kind: "CLICK".to_owned(),
            label: "Go".to_owned(),
            role: Some("button".to_owned()),
            region: None,
            fingerprint: 0,
            state: None,
        }]);
        let space = action_space(&line);
        let click = space.get_str("CLICK:go").unwrap();
        assert!(click.target().is_none(), "pre-region diaries degrade");
        // ...and are absent from the target-bound candidate set.
        assert_eq!(space.targets_of(ActionKind::Click).count(), 0);
    }
}
