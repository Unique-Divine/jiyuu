//! Forward-row decisions for GraphQL cross-repo linking.

/// ABI mirror of `cbm_cross_route_action_t` in `pass_cross_repo.h`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub(crate) enum Action {
    Keep,
    Drop,
    LinkAmbiguous,
    LinkResolved,
}

/// Provider-side calls reach this function only for an existing row owned by
/// that provider. An ambiguous caller-side candidate can replace another
/// ambiguous row, but never a resolved row owned by another target.
pub(crate) extern "C" fn plan_link(
    provider_side: bool,
    has_handler: bool,
    ambiguous: bool,
    names_other: bool,
    existing_ambiguous: bool,
) -> Action {
    match (has_handler, ambiguous) {
        (false, _) if provider_side => Action::Drop,
        (false, _) => Action::Keep,
        (true, false) => Action::LinkResolved,
        (true, true) if !provider_side && names_other && !existing_ambiguous => {
            Action::Keep
        }
        (true, true) => Action::LinkAmbiguous,
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, plan_link};

    #[test]
    fn caller_preserves_a_resolved_row_when_another_target_is_ambiguous() {
        assert_eq!(plan_link(false, true, true, true, false), Action::Keep);
        assert_eq!(
            plan_link(false, true, true, true, true),
            Action::LinkAmbiguous
        );
        assert_eq!(
            plan_link(false, true, false, true, true),
            Action::LinkResolved
        );
    }

    #[test]
    fn only_provider_relinks_drop_a_forward_row_without_a_handler() {
        assert_eq!(plan_link(true, false, false, false, false), Action::Drop);
        assert_eq!(plan_link(false, false, false, false, false), Action::Keep);
        assert_eq!(
            plan_link(true, true, true, false, false),
            Action::LinkAmbiguous
        );
    }
}
