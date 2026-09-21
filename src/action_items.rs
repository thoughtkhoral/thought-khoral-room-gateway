#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionItem {
    pub text: String,
    pub owner: Option<String>,
    pub due: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    EmptyField,
    UnsupportedField,
}

pub fn extract_action_items(source: &str) -> Result<Vec<ActionItem>, ParseError> {
    let source = source.replacen("@action-items", "", 1);
    let mut items = Vec::new();
    for line in source.lines() {
        let Some(line) = line.trim_start().strip_prefix("- ") else {
            continue;
        };
        let mut fields = line.split('|').map(str::trim);
        let text = fields.next().unwrap_or_default().trim();
        if text.is_empty() {
            return Err(ParseError::EmptyField);
        }
        let mut owner = None;
        let mut due = None;
        for field in fields {
            let (name, value) = field.split_once(':').ok_or(ParseError::UnsupportedField)?;
            let value = value.trim();
            if value.is_empty() {
                return Err(ParseError::EmptyField);
            }
            match name.trim() {
                "owner" if owner.is_none() => owner = Some(value.to_owned()),
                "due" if due.is_none() => due = Some(value.to_owned()),
                _ => return Err(ParseError::UnsupportedField),
            }
        }
        items.push(ActionItem {
            text: text.to_owned(),
            owner,
            due,
        });
    }
    Ok(items)
}
use uuid::Uuid;

pub const ACTION_ITEMS_AGENT_ID: Uuid = Uuid::from_u128(0x74686f756768746b_686f72616c000002);
pub const ACTION_ITEMS_AGENT_DISPLAY_NAME: &str = "Action Items";
