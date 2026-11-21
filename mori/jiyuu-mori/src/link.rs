//! Choose the schema field and handler for one GraphQL Route.

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct RoutePlan<T> {
    pub field_index: usize,
    pub resolver: Option<T>,
    pub ambiguous: bool,
}

impl<T: Copy + Eq> RoutePlan<T> {
    pub fn new(field_count: usize) -> Self {
        Self {
            field_index: 0,
            resolver: None,
            ambiguous: field_count > 1,
        }
    }

    /// Choose the sole owning resolver. Distinct owning resolvers make the
    /// plan ambiguous and use the first schema field as the handler.
    /// Returns whether the caller should visit more candidates.
    pub fn consider(&mut self, field_index: usize, resolver: T) -> bool {
        match self.resolver {
            Some(selected) if selected == resolver => true,
            Some(_) => {
                *self = Self {
                    field_index: 0,
                    resolver: None,
                    ambiguous: true,
                };
                false
            }
            None => {
                *self = Self {
                    field_index,
                    resolver: Some(resolver),
                    ambiguous: false,
                };
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RoutePlan;

    #[test]
    fn schema_fields_handle_routes_without_an_owning_resolver() {
        assert!(!RoutePlan::<i32>::new(1).ambiguous);
        assert_eq!(
            RoutePlan::<i32>::new(2),
            RoutePlan {
                field_index: 0,
                resolver: None,
                ambiguous: true
            }
        );
    }

    #[test]
    fn chooses_one_resolver_then_falls_back_on_a_distinct_resolver() {
        let mut plan = RoutePlan::new(3);
        assert!(plan.consider(1, 10));
        assert!(plan.consider(2, 10));
        assert_eq!(
            plan,
            RoutePlan {
                field_index: 1,
                resolver: Some(10),
                ambiguous: false
            }
        );
        assert!(!plan.consider(2, 20));
        assert_eq!(plan, RoutePlan::new(3));
    }
}
