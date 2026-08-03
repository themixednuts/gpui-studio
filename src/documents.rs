use crate::ElementSelection;

/// Stable identity for an editor document independent of its rendered label.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DocumentId {
    Main,
    Component(String),
}

#[derive(Clone, Debug, PartialEq)]
struct OpenDocument {
    id: DocumentId,
    selection: ElementSelection,
}

/// Ordered open documents with one selection retained per tab.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DocumentTabs {
    open: Vec<OpenDocument>,
    active: DocumentId,
}

impl DocumentTabs {
    pub(crate) fn new(main_selection: ElementSelection) -> Self {
        Self {
            open: vec![OpenDocument {
                id: DocumentId::Main,
                selection: main_selection,
            }],
            active: DocumentId::Main,
        }
    }

    pub(crate) fn open_ids(&self) -> impl Iterator<Item = &DocumentId> {
        self.open.iter().map(|document| &document.id)
    }

    pub(crate) fn active_id(&self) -> &DocumentId {
        &self.active
    }

    pub(crate) fn replace_active_selection(&mut self, selection: ElementSelection) {
        if let Some(document) = self
            .open
            .iter_mut()
            .find(|document| document.id == self.active)
        {
            document.selection = selection;
        }
    }

    pub(crate) fn activate(
        &mut self,
        id: &DocumentId,
        current_selection: ElementSelection,
    ) -> Option<ElementSelection> {
        self.replace_active_selection(current_selection);
        let selection = self
            .open
            .iter()
            .find(|document| &document.id == id)?
            .selection
            .clone();
        self.active = id.clone();
        Some(selection)
    }

    pub(crate) fn open_component(
        &mut self,
        component_id: impl Into<String>,
        current_selection: ElementSelection,
    ) -> ElementSelection {
        self.replace_active_selection(current_selection);
        let component_id = component_id.into();
        let id = DocumentId::Component(component_id.clone());
        if let Some(document) = self.open.iter().find(|document| document.id == id) {
            self.active = id;
            return document.selection.clone();
        }

        let selection =
            ElementSelection::new("root", format!("component/{component_id}/root"), 1, None);
        self.open.push(OpenDocument {
            id: id.clone(),
            selection: selection.clone(),
        });
        self.active = id;
        selection
    }

    pub(crate) fn close(
        &mut self,
        id: &DocumentId,
        current_selection: ElementSelection,
    ) -> Option<ElementSelection> {
        if matches!(id, DocumentId::Main) {
            return None;
        }
        self.replace_active_selection(current_selection);
        let index = self.open.iter().position(|document| &document.id == id)?;
        let closing_active = self.active == *id;
        self.open.remove(index);
        if closing_active {
            let next_index = index.saturating_sub(1).min(self.open.len() - 1);
            self.active = self.open[next_index].id.clone();
        }
        self.open
            .iter()
            .find(|document| document.id == self.active)
            .map(|document| document.selection.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{DocumentId, DocumentTabs};
    use crate::ElementSelection;

    fn selection(id: &str) -> ElementSelection {
        ElementSelection::new(id, format!("runtime/{id}"), 1, None)
    }

    #[test]
    fn component_tabs_are_deduplicated_and_retain_independent_selections() {
        let mut tabs = DocumentTabs::new(selection("main-root"));
        let component_root = tabs.open_component("button", selection("main-selection"));
        assert_eq!(component_root.authored_id, "root");
        assert_eq!(tabs.open_ids().count(), 2);

        let main = tabs.activate(&DocumentId::Main, selection("button-label"));
        assert_eq!(main, Some(selection("main-selection")));
        let restored = tabs.open_component("button", selection("new-main-selection"));
        assert_eq!(restored, selection("button-label"));
        assert_eq!(tabs.open_ids().count(), 2);
    }

    #[test]
    fn closing_active_component_returns_to_its_left_neighbor_and_main_is_permanent() {
        let mut tabs = DocumentTabs::new(selection("main"));
        tabs.open_component("button", selection("main"));
        tabs.open_component("card", selection("button"));

        let restored = tabs.close(&DocumentId::Component("card".to_owned()), selection("card"));
        assert_eq!(restored, Some(selection("button")));
        assert_eq!(
            tabs.active_id(),
            &DocumentId::Component("button".to_owned())
        );
        assert_eq!(tabs.close(&DocumentId::Main, selection("button")), None);
        assert_eq!(tabs.open_ids().count(), 2);
    }
}
