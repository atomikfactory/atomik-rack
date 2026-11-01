//! The flattened folder tree: sections, tags, bookmarks and drives as a list of visible items.

use super::{PlacesData, TreeChild};
use crate::action::Place;
use atomik_library::LibrarySection;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Deepest folder level the tree flattens.
const MAX_DEPTH: u8 = 32;

/// A section heading of the tree (not clickable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Heading {
    /// The library sections.
    Library,
    /// Bookmarked folders.
    Bookmarks,
    /// Drives and the home folder.
    Locations,
}

/// What a tree item stands for (its identity across rebuilds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Key {
    /// A section heading.
    Heading(Heading),
    /// One library section.
    Library(LibrarySection),
    /// The Favourites entry.
    Favorites,
    /// One named favourite list.
    List(String),
    /// The expandable Tags entry.
    TagsHeader,
    /// One tag.
    Tag(String),
    /// A folder; `bookmark` is true for a bookmark root.
    Folder {
        /// Absolute path.
        path: PathBuf,
        /// Whether it is a top-level bookmark entry.
        bookmark: bool,
    },
}

impl Key {
    /// The place that clicking the item shows.
    pub fn place(&self) -> Option<Place> {
        match self {
            Key::Library(section) => Some(Place::Library(*section)),
            Key::Favorites => Some(Place::Favorites),
            Key::List(name) => Some(Place::List(name.clone())),
            Key::Tag(t) => Some(Place::Tag(t.clone())),
            Key::Folder { path, .. } => Some(Place::Folder(path.clone())),
            Key::Heading(_) | Key::TagsHeader => None,
        }
    }

    /// Whether the item can hold the cursor.
    pub fn selectable(&self) -> bool {
        !matches!(self, Key::Heading(_))
    }
}

/// One visible row of the tree.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Item {
    /// Identity.
    pub key: Key,
    /// Nesting level (indentation).
    pub depth: u8,
    /// Shown text.
    pub label: String,
    /// Count shown after the label.
    pub count: Option<usize>,
    /// Whether a disclosure arrow is shown.
    pub expandable: bool,
    /// Whether the children are shown.
    pub expanded: bool,
    /// Expanded but the children are not known yet.
    pub loading: bool,
}

/// The inputs of [`build`].
pub(super) struct Source<'a> {
    /// The roots.
    pub places: &'a PlacesData,
    /// Known sub-folders by parent.
    pub children: &'a BTreeMap<PathBuf, Vec<TreeChild>>,
    /// Expanded folders.
    pub expanded: &'a BTreeSet<PathBuf>,
    /// Whether the Tags entry is open.
    pub tags_open: bool,
}

fn folder_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn push_folder(
    out: &mut Vec<Item>,
    src: &Source<'_>,
    path: &Path,
    label: String,
    depth: u8,
    bookmark: bool,
) {
    let known = src.children.get(path);
    let expandable = known.is_none_or(|c| !c.is_empty());
    let expanded = expandable && src.expanded.contains(path);
    out.push(Item {
        key: Key::Folder {
            path: path.to_path_buf(),
            bookmark,
        },
        depth,
        label,
        count: None,
        expandable,
        expanded,
        loading: expanded && known.is_none(),
    });
    if expanded && depth < MAX_DEPTH {
        for child in known.into_iter().flatten() {
            push_folder(out, src, &child.path, child.name.clone(), depth + 1, false);
        }
    }
}

/// Flattens the tree into its visible items.
pub(super) fn build(src: &Source<'_>) -> Vec<Item> {
    let places = src.places;
    let mut out = vec![Item {
        key: Key::Heading(Heading::Library),
        depth: 0,
        label: "Library".into(),
        count: None,
        expandable: false,
        expanded: false,
        loading: false,
    }];
    for section in LibrarySection::ALL {
        out.push(Item {
            key: Key::Library(section),
            depth: 0,
            label: section.title().into(),
            count: None,
            expandable: false,
            expanded: false,
            loading: false,
        });
    }
    out.push(Item {
        key: Key::Favorites,
        depth: 0,
        label: "Favourites".into(),
        count: Some(places.favorites),
        expandable: false,
        expanded: false,
        loading: false,
    });
    for (name, n) in &places.lists {
        out.push(Item {
            key: Key::List(name.clone()),
            depth: 1,
            label: name.clone(),
            count: Some(*n),
            expandable: false,
            expanded: false,
            loading: false,
        });
    }
    let tags_open = src.tags_open && !places.tags.is_empty();
    out.push(Item {
        key: Key::TagsHeader,
        depth: 0,
        label: "Tags".into(),
        count: Some(places.tags.len()),
        expandable: !places.tags.is_empty(),
        expanded: tags_open,
        loading: false,
    });
    if tags_open {
        for (name, n) in &places.tags {
            out.push(Item {
                key: Key::Tag(name.clone()),
                depth: 1,
                label: name.clone(),
                count: Some(*n),
                expandable: false,
                expanded: false,
                loading: false,
            });
        }
    }
    let mut section = |heading: Heading, label: &str, paths: &[PathBuf], bookmark: bool| {
        if paths.is_empty() {
            return;
        }
        out.push(Item {
            key: Key::Heading(heading),
            depth: 0,
            label: label.into(),
            count: None,
            expandable: false,
            expanded: false,
            loading: false,
        });
        for p in paths {
            push_folder(&mut out, src, p, folder_label(p), 0, bookmark);
        }
    };
    section(Heading::Bookmarks, "Bookmarks", &places.bookmarks, true);
    section(Heading::Locations, "Locations", &places.roots, false);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places() -> PlacesData {
        PlacesData {
            favorites: 3,
            tags: vec![("808".into(), 4), ("snare".into(), 2)],
            lists: Vec::new(),
            bookmarks: vec![PathBuf::from("/music/drums")],
            roots: vec![PathBuf::from("/"), PathBuf::from("/home/me")],
        }
    }

    fn flat(
        p: &PlacesData,
        children: &BTreeMap<PathBuf, Vec<TreeChild>>,
        expanded: &BTreeSet<PathBuf>,
        tags_open: bool,
    ) -> Vec<Item> {
        build(&Source {
            places: p,
            children,
            expanded,
            tags_open,
        })
    }

    #[test]
    fn sections_come_in_order_with_counts() {
        let items = flat(&places(), &BTreeMap::new(), &BTreeSet::new(), true);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Library",
                "Devices",
                "Patches",
                "Containers",
                "Clips",
                "Projects",
                "Favourites",
                "Tags",
                "808",
                "snare",
                "Bookmarks",
                "drums",
                "Locations",
                "/",
                "me"
            ]
        );
        assert_eq!(items[6].count, Some(3));
        assert_eq!(items[8].count, Some(4));
        assert!(items[8].depth == 1 && items[11].depth == 0);
        assert!(matches!(items[11].key, Key::Folder { bookmark: true, .. }));
    }

    #[test]
    fn the_library_block_comes_first_with_its_sections_in_order() {
        let items = flat(&places(), &BTreeMap::new(), &BTreeSet::new(), true);
        assert_eq!(items[0].key, Key::Heading(Heading::Library));
        let sections: Vec<&Key> = items[1..6].iter().map(|i| &i.key).collect();
        let expected: Vec<Key> = LibrarySection::ALL.into_iter().map(Key::Library).collect();
        assert_eq!(sections, expected.iter().collect::<Vec<_>>());
    }

    #[test]
    fn closed_tags_hide_their_children() {
        let items = flat(&places(), &BTreeMap::new(), &BTreeSet::new(), false);
        assert!(!items.iter().any(|i| matches!(i.key, Key::Tag(_))));
        assert!(!items[7].expanded && items[7].expandable);
    }

    #[test]
    fn empty_sections_are_left_out() {
        let items = flat(
            &PlacesData::default(),
            &BTreeMap::new(),
            &BTreeSet::new(),
            true,
        );
        // The library block, Favourites and Tags stay.
        assert_eq!(items.len(), 8);
        assert!(!items[7].expandable);
    }

    #[test]
    fn unknown_children_show_loading_and_empty_children_make_a_leaf() {
        let p = places();
        let mut expanded = BTreeSet::new();
        expanded.insert(PathBuf::from("/music/drums"));
        let items = flat(&p, &BTreeMap::new(), &expanded, true);
        let drums = items.iter().find(|i| i.label == "drums").unwrap();
        assert!(drums.expandable && drums.expanded && drums.loading);

        let mut kids = BTreeMap::new();
        kids.insert(PathBuf::from("/music/drums"), Vec::new());
        let items = flat(&p, &kids, &expanded, true);
        let drums = items.iter().find(|i| i.label == "drums").unwrap();
        assert!(!drums.expandable && !drums.expanded && !drums.loading);
    }

    #[test]
    fn known_children_nest_under_their_parent() {
        let p = places();
        let mut expanded = BTreeSet::new();
        expanded.insert(PathBuf::from("/music/drums"));
        let mut kids = BTreeMap::new();
        kids.insert(
            PathBuf::from("/music/drums"),
            vec![TreeChild {
                name: "kicks".into(),
                path: PathBuf::from("/music/drums/kicks"),
            }],
        );
        let items = flat(&p, &kids, &expanded, true);
        let i = items.iter().position(|i| i.label == "drums").unwrap();
        assert_eq!(items[i + 1].label, "kicks");
        assert_eq!(items[i + 1].depth, 1);
        assert!(matches!(
            items[i + 1].key,
            Key::Folder {
                bookmark: false,
                ..
            }
        ));
    }

    #[test]
    fn keys_map_to_places() {
        assert_eq!(Key::Favorites.place(), Some(Place::Favorites));
        assert_eq!(Key::Tag("x".into()).place(), Some(Place::Tag("x".into())));
        assert_eq!(Key::TagsHeader.place(), None);
        assert_eq!(
            Key::Library(LibrarySection::Clips).place(),
            Some(Place::Library(LibrarySection::Clips))
        );
        assert!(!Key::Heading(Heading::Bookmarks).selectable());
        assert!(!Key::Heading(Heading::Library).selectable());
        assert!(Key::Library(LibrarySection::Devices).selectable());
    }
}
