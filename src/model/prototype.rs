//! Prototype links: clicking a layer in present mode navigates to an artboard.
//!
//! Links are attributes on the layer (`data-link="n12"` or `"back"`, plus
//! `data-transition`), so they travel with copy/paste and component
//! instances. Code export strips them.

use super::{Document, NodeId};

/// Target artboard id, or `back`.
pub const LINK_ATTR: &str = "data-link";
/// How the next screen appears.
pub const TRANSITION_ATTR: &str = "data-transition";
/// Editor-only prototype attributes.
pub const PROTOTYPE_ATTRS: [&str; 2] = [LINK_ATTR, TRANSITION_ATTR];

/// Where a link goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// Navigate to an artboard (by root id).
    Artboard(NodeId),
    /// Return to the previous screen.
    Back,
}

impl LinkTarget {
    /// Attribute value.
    #[must_use]
    pub fn to_attr(self) -> String {
        match self {
            Self::Artboard(id) => id.to_string(),
            Self::Back => "back".to_owned(),
        }
    }

    /// Parse an attribute value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        if value.trim() == "back" {
            Some(Self::Back)
        } else {
            NodeId::parse(value).map(Self::Artboard)
        }
    }
}

/// Screen transition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transition {
    /// Swap immediately.
    #[default]
    Instant,
    /// Fade the new screen in.
    Dissolve,
    /// Push in from the right.
    SlideLeft,
    /// Push in from the left.
    SlideRight,
}

impl Transition {
    /// Every transition, in picker order.
    pub const ALL: [Self; 4] = [
        Self::Instant,
        Self::Dissolve,
        Self::SlideLeft,
        Self::SlideRight,
    ];

    /// Attribute value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Instant => "instant",
            Self::Dissolve => "dissolve",
            Self::SlideLeft => "slide-left",
            Self::SlideRight => "slide-right",
        }
    }

    /// Display label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Instant => "Instant",
            Self::Dissolve => "Dissolve",
            Self::SlideLeft => "Slide ←",
            Self::SlideRight => "Slide →",
        }
    }

    /// Parse an attribute value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == value.trim())
    }

    /// The transition that undoes this one (for going back).
    #[must_use]
    pub fn reversed(self) -> Self {
        match self {
            Self::SlideLeft => Self::SlideRight,
            Self::SlideRight => Self::SlideLeft,
            other => other,
        }
    }
}

/// A resolved link on a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    /// The layer carrying the link.
    pub source: NodeId,
    /// Where it goes.
    pub target: LinkTarget,
    /// How.
    pub transition: Transition,
}

impl Document {
    /// The link declared on exactly this layer, when its target still exists.
    #[must_use]
    pub fn link_of(&self, id: NodeId) -> Option<Link> {
        let node = self.get(id)?;
        let target = LinkTarget::parse(node.attr(LINK_ATTR)?)?;
        if let LinkTarget::Artboard(board) = target
            && !self.is_artboard(board)
        {
            return None;
        }
        let transition = node
            .attr(TRANSITION_ATTR)
            .and_then(Transition::parse)
            .unwrap_or_default();
        Some(Link {
            source: id,
            target,
            transition,
        })
    }

    /// The link that a click on `id` follows: its own, or its nearest linked
    /// ancestor's (like a click bubbling to an enclosing `<a>`).
    #[must_use]
    pub fn link_at(&self, id: NodeId) -> Option<Link> {
        std::iter::once(id)
            .chain(self.ancestors(id))
            .find_map(|n| self.link_of(n))
    }

    /// Every live link on a page, in document order.
    #[must_use]
    pub fn page_links(&self, page: usize) -> Vec<Link> {
        let Some(page) = self.pages.get(page) else {
            return Vec::new();
        };
        page.artboards
            .iter()
            .flat_map(|a| self.descendants(a.root))
            .filter_map(|id| self.link_of(id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_resolve_bubble_and_ignore_dead_targets() {
        let mut doc = Document::new();
        let home = doc.create_artboard(0, "Home", (0.0, 0.0), (400.0, 300.0));
        let detail = doc.create_artboard(0, "Detail", (500.0, 0.0), (400.0, 300.0));
        let button = doc.new_element("button");
        doc.attach(button, home, None).unwrap();
        let label = doc.new_element("span");
        doc.attach(label, button, None).unwrap();
        let node = doc.get_mut(button).unwrap();
        node.set_attr(LINK_ATTR, &LinkTarget::Artboard(detail).to_attr());
        node.set_attr(TRANSITION_ATTR, Transition::SlideLeft.as_str());
        let link = doc.link_at(label).unwrap();
        assert_eq!(link.source, button);
        assert_eq!(link.target, LinkTarget::Artboard(detail));
        assert_eq!(link.transition, Transition::SlideLeft);
        assert_eq!(link.transition.reversed(), Transition::SlideRight);
        assert_eq!(doc.page_links(0).len(), 1);
        doc.get_mut(button).unwrap().set_attr(LINK_ATTR, "n9999");
        assert_eq!(doc.link_at(label), None, "dead targets are ignored");
        doc.get_mut(button).unwrap().set_attr(LINK_ATTR, "back");
        assert_eq!(doc.link_at(label).unwrap().target, LinkTarget::Back);
    }
}
