use std::fmt;

use crate::error::CoreError;

/// Control role. Unknown platform roles collapse to [`Role::Generic`] at the
/// snapshot boundary rather than being invented here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Role {
    Button,
    Link,
    Text,
    TextField,
    Checkbox,
    MenuItem,
    Navigation,
    Image,
    Generic,
    Slider,
    Tab,
    Heading,
    /// A dialog or alert dialog. With [`RegionFlags::modal`] it blocks input
    /// to every region outside it. See `aui-guard`'s front layer.
    Dialog,
    /// ARIA combobox / autocomplete field (type + optional select/click).
    ComboBox,
    /// ARIA listbox container for options (visible window only).
    ListBox,
    /// ARIA option inside a listbox / popup (click / select).
    Option,
}

impl Role {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Button => "button",
            Self::Link => "link",
            Self::Text => "text",
            Self::TextField => "text_field",
            Self::Checkbox => "checkbox",
            Self::MenuItem => "menu_item",
            Self::Navigation => "navigation",
            Self::Image => "image",
            Self::Generic => "generic",
            Self::Slider => "slider",
            Self::Tab => "tab",
            Self::Heading => "heading",
            Self::Dialog => "dialog",
            Self::ComboBox => "combobox",
            Self::ListBox => "listbox",
            Self::Option => "option",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "button" => Self::Button,
            "link" => Self::Link,
            "text" => Self::Text,
            "text_field" | "textfield" => Self::TextField,
            "checkbox" => Self::Checkbox,
            "menu_item" | "menuitem" => Self::MenuItem,
            "navigation" | "nav" => Self::Navigation,
            "image" => Self::Image,
            "generic" => Self::Generic,
            "slider" => Self::Slider,
            "tab" => Self::Tab,
            "heading" => Self::Heading,
            "dialog" | "alertdialog" | "alert_dialog" => Self::Dialog,
            "combobox" | "combo_box" => Self::ComboBox,
            "listbox" | "list_box" => Self::ListBox,
            "option" => Self::Option,
            _ => return None,
        })
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An action a region claims it can accept. Claiming an action is not the
/// same as being allowed to run it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Action {
    Click,
    Type,
    Scroll,
    Focus,
    Hover,
    Select,
    Toggle,
    /// Press, move, and release: the region can be dragged.
    Drag,
}

impl Action {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Type => "type",
            Self::Scroll => "scroll",
            Self::Focus => "focus",
            Self::Hover => "hover",
            Self::Select => "select",
            Self::Toggle => "toggle",
            Self::Drag => "drag",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "click" => Self::Click,
            "type" => Self::Type,
            "scroll" => Self::Scroll,
            "focus" => Self::Focus,
            "hover" => Self::Hover,
            "select" => Self::Select,
            "toggle" => Self::Toggle,
            "drag" => Self::Drag,
            _ => return None,
        })
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Directed relation from one region to another.
///
/// Geometric variants are decided in `aui-geometry`. `Parent` and
/// `Child` come from the explicit parent link on the region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Relation {
    Parent,
    Child,
    Above,
    Below,
    Inside,
    Contains,
    Overlaps,
    Near,
    AlignedX,
    AlignedY,
}

impl Relation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Parent => "parent",
            Self::Child => "child",
            Self::Above => "above",
            Self::Below => "below",
            Self::Inside => "inside",
            Self::Contains => "contains",
            Self::Overlaps => "overlaps",
            Self::Near => "near",
            Self::AlignedX => "aligned_x",
            Self::AlignedY => "aligned_y",
        }
    }
}

impl fmt::Display for Relation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Coarse position of a region's center inside the viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Zone {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

impl Zone {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Center => "center",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "left" => Self::Left,
            "right" => Self::Right,
            "top" => Self::Top,
            "bottom" => Self::Bottom,
            "center" => Self::Center,
            _ => return None,
        })
    }
}

impl fmt::Display for Zone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Area class of a region relative to the viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum SizeClass {
    Small,
    Medium,
    Large,
}

impl SizeClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
}

impl fmt::Display for SizeClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which observers reported a region. Bits outside [`SourceMask::ALL`] are rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceMask(u8);

impl SourceMask {
    pub const DOM: Self = Self(1 << 0);
    pub const ACCESSIBILITY: Self = Self(1 << 1);
    pub const SCREENSHOT: Self = Self(1 << 2);
    pub const CUA: Self = Self(1 << 3);
    pub const ALL: Self = Self(0b1111);
    pub const NONE: Self = Self(0);

    pub const KNOWN_COUNT: u32 = 4;

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn count(self) -> u32 {
        self.0.count_ones()
    }

    pub const fn try_from_bits(bits: u8) -> Result<Self, CoreError> {
        if bits & !Self::ALL.0 == 0 {
            Ok(Self(bits))
        } else {
            Err(CoreError::UnknownSourceBits(bits))
        }
    }

    pub fn iter(self) -> impl Iterator<Item = Self> {
        [Self::DOM, Self::ACCESSIBILITY, Self::SCREENSHOT, Self::CUA]
            .into_iter()
            .filter(move |bit| self.contains(*bit))
    }

    pub fn parse_list(raw: &str) -> Result<Self, String> {
        let mut mask = Self::NONE;
        if raw.is_empty() {
            return Ok(mask);
        }
        for part in raw.split(',') {
            let part = part.trim();
            let bit = match part {
                "dom" => Self::DOM,
                "accessibility" | "ax" => Self::ACCESSIBILITY,
                "screenshot" => Self::SCREENSHOT,
                "cua" => Self::CUA,
                other => return Err(format!("unknown source `{other}`")),
            };
            mask = mask.union(bit);
        }
        Ok(mask)
    }
}

/// Independent condition flags. These are not a lifecycle; a control may be
/// disabled and offscreen at the same time. Each one is a locate penalty,
/// except `modal` (dialog layer marker, never a penalty on the dialog) and
/// `readonly` (TYPE/SELECT refuse at the hard gate; not a locate penalty).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RegionFlags {
    disabled: bool,
    hidden: bool,
    occluded: bool,
    offscreen: bool,
    stale: bool,
    ambiguous: bool,
    detached: bool,
    modal: bool,
    /// HTML `readonly` / `aria-readonly="true"`. Observable; the hard gate
    /// refuses TYPE_TEXT / SELECT on a readonly control before any CDP input.
    readonly: bool,
}

impl RegionFlags {
    pub const fn none() -> Self {
        Self {
            disabled: false,
            hidden: false,
            occluded: false,
            offscreen: false,
            stale: false,
            ambiguous: false,
            detached: false,
            modal: false,
            readonly: false,
        }
    }

    pub const fn disabled(self) -> bool {
        self.disabled
    }
    pub const fn hidden(self) -> bool {
        self.hidden
    }
    pub const fn occluded(self) -> bool {
        self.occluded
    }
    pub const fn offscreen(self) -> bool {
        self.offscreen
    }
    pub const fn stale(self) -> bool {
        self.stale
    }
    pub const fn ambiguous(self) -> bool {
        self.ambiguous
    }
    pub const fn detached(self) -> bool {
        self.detached
    }
    pub const fn modal(self) -> bool {
        self.modal
    }
    pub const fn readonly(self) -> bool {
        self.readonly
    }

    pub fn set_disabled(&mut self, value: bool) {
        self.disabled = value;
    }
    pub fn set_hidden(&mut self, value: bool) {
        self.hidden = value;
    }
    pub fn set_occluded(&mut self, value: bool) {
        self.occluded = value;
    }
    pub fn set_offscreen(&mut self, value: bool) {
        self.offscreen = value;
    }
    pub fn set_stale(&mut self, value: bool) {
        self.stale = value;
    }
    pub fn set_ambiguous(&mut self, value: bool) {
        self.ambiguous = value;
    }
    pub fn set_detached(&mut self, value: bool) {
        self.detached = value;
    }
    pub fn set_modal(&mut self, value: bool) {
        self.modal = value;
    }
    pub fn set_readonly(&mut self, value: bool) {
        self.readonly = value;
    }

    pub fn parse_list(raw: &str) -> Result<Self, String> {
        let mut flags = Self::none();
        if raw.is_empty() {
            return Ok(flags);
        }
        for part in raw.split(',') {
            match part.trim() {
                "disabled" => flags.disabled = true,
                "hidden" => flags.hidden = true,
                "occluded" => flags.occluded = true,
                "offscreen" => flags.offscreen = true,
                "stale" => flags.stale = true,
                "ambiguous" => flags.ambiguous = true,
                "detached" => flags.detached = true,
                "modal" => flags.modal = true,
                "readonly" => flags.readonly = true,
                other => return Err(format!("unknown flag `{other}`")),
            }
        }
        Ok(flags)
    }

    pub const fn bits(self) -> u16 {
        let mut bits = 0u16;
        if self.disabled {
            bits |= 1 << 0;
        }
        if self.hidden {
            bits |= 1 << 1;
        }
        if self.occluded {
            bits |= 1 << 2;
        }
        if self.offscreen {
            bits |= 1 << 3;
        }
        if self.stale {
            bits |= 1 << 4;
        }
        if self.ambiguous {
            bits |= 1 << 5;
        }
        if self.detached {
            bits |= 1 << 6;
        }
        if self.modal {
            bits |= 1 << 7;
        }
        if self.readonly {
            bits |= 1 << 8;
        }
        bits
    }
}
