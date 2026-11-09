/// Observed control state. Evidence only: it is NOT part of the region
/// fingerprint, so a value change never makes a ticket stale.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElementState {
    /// INPUT (except type password / hidden / file / checkbox / radio) or TEXTAREA value, ≤ 200 chars.
    pub value: Option<String>,
    /// checkbox / radio `.checked`; otherwise `aria-checked` "true"/"false" ("mixed" → None).
    pub checked: Option<bool>,
    /// `aria-expanded` "true"/"false".
    pub expanded: Option<bool>,
    /// SELECT: labels of the selected options, trimmed, joined with ", ".
    pub selected: Option<String>,
    /// SELECT: enabled option labels in document order (skip disabled options and options in a disabled optgroup), trimmed, each ≤ 80 chars, at most 50.
    pub options: Vec<String>,
    /// INPUT's `type` attribute, lowercased. Evidence only — it marks
    /// sensitive fields (`password`) without ever reading their value.
    pub input_type: Option<String>,
    /// A element's resolved `href`, ≤ 500 chars. Evidence only — two links
    /// with one label and one destination are the same choice.
    pub href: Option<String>,
    /// A form control's form, as Enter would submit it: evidence only, read
    /// for INPUT / TEXTAREA / SELECT / BUTTON. `None`: not read.
    pub form: Option<FormSubmit>,
}

/// The form a control belongs to, and the button implicit submission
/// (Enter in a field) would press: the form's first submit button
/// (`button` / `input type=submit|image`) in tree order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormSubmit {
    /// Enter here submits no form: the control is in none, or it is a
    /// text area or a select.
    NotInForm,
    /// In a form with no submit button.
    NoSubmitButton,
    /// That button's label (aria-label, else text, else value / alt / title).
    Button(String),
}

impl ElementState {
    pub fn is_empty(&self) -> bool {
        self.value.is_none()
            && self.checked.is_none()
            && self.expanded.is_none()
            && self.selected.is_none()
            && self.options.is_empty()
            && self.input_type.is_none()
            && self.href.is_none()
            && self.form.is_none()
    }
}
