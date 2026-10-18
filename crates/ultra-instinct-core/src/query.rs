use crate::error::CoreError;
use crate::id::RegionId;
use crate::text::tokenize;
use crate::vocab::{Action, Role, Zone};

/// A structured locate request. Absent fields are unconstrained.
///
/// Text, when present, must contain at least one alphanumeric token. The same
/// query value is pure data: ranking code is not allowed to consult a clock
/// or a random source.
///
/// Two fields carry world context instead of describing the target itself:
///
/// - `within`: the target must sit under this container in the parent chain
///   (an ancestor, not the container itself). A hard constraint.
/// - `near`: an anchor such as the focused region (the "cursor"). Locate
///   scopes to the innermost ancestor-or-self of the anchor that contains a
///   best text/role match. With no anchor, or no such ancestor, locate falls
///   back to the unscoped ranking, the way a cargo runner falls back to the
///   default command when there is no cursor context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocateQuery {
    text: Option<String>,
    role: Option<Role>,
    position: Option<Zone>,
    action: Option<Action>,
    within: Option<RegionId>,
    near: Option<RegionId>,
}

impl LocateQuery {
    pub const fn new() -> Self {
        Self {
            text: None,
            role: None,
            position: None,
            action: None,
            within: None,
            near: None,
        }
    }

    pub fn text(mut self, text: impl Into<String>) -> Result<Self, CoreError> {
        let text = text.into();
        if tokenize(&text).is_empty() {
            return Err(CoreError::EmptyQueryText);
        }
        self.text = Some(text);
        Ok(self)
    }

    pub fn role(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }

    pub fn position(mut self, position: Zone) -> Self {
        self.position = Some(position);
        self
    }

    pub fn action(mut self, action: Action) -> Self {
        self.action = Some(action);
        self
    }

    /// Require the target to be a descendant of `container`.
    pub fn within(mut self, container: RegionId) -> Self {
        self.within = Some(container);
        self
    }

    /// Scope the ranking to the context of `anchor` (for example the focused
    /// region). `None` clears the anchor, which is the default ranking.
    pub fn near(mut self, anchor: Option<RegionId>) -> Self {
        self.near = anchor;
        self
    }

    pub fn text_ref(&self) -> Option<&str> {
        self.text.as_deref()
    }

    pub const fn role_ref(&self) -> Option<Role> {
        self.role
    }

    pub const fn position_ref(&self) -> Option<Zone> {
        self.position
    }

    pub const fn action_ref(&self) -> Option<Action> {
        self.action
    }

    pub const fn within_ref(&self) -> Option<&RegionId> {
        self.within.as_ref()
    }

    pub const fn near_ref(&self) -> Option<&RegionId> {
        self.near.as_ref()
    }
}

impl Default for LocateQuery {
    fn default() -> Self {
        Self::new()
    }
}
