//! The workspace: an opened folder plus the lazy file tree browsing it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One entry in the file tree. Directory children are loaded lazily,
/// only when the folder is first expanded — never recursive at startup.
#[derive(Debug, Clone)]
pub struct Node {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub expanded: bool,
    /// `Some` once expanded; `None` for files and unexpanded directories.
    pub children: Option<Vec<Node>>,
    /// The directory could not be read (permissions, gone, ...).
    pub unreadable: bool,
}

/// An opened folder and its file tree.
#[derive(Debug)]
pub struct Workspace {
    pub root: PathBuf,
    /// Top-level entries.
    pub nodes: Vec<Node>,
    /// The entry selected in the UI, if any.
    pub selected: Option<PathBuf>,
}

/// Collects the paths of every expanded directory (any depth).
fn collect_expanded(nodes: &[Node], out: &mut HashSet<PathBuf>) {
    for node in nodes {
        if node.is_dir && node.expanded {
            out.insert(node.path.clone());
            if let Some(children) = &node.children {
                collect_expanded(children, out);
            }
        }
    }
}

/// Re-opens (and lazily reloads) directories that were previously expanded.
fn reexpand(nodes: &mut [Node], expanded: &HashSet<PathBuf>) {
    for node in nodes.iter_mut() {
        if node.is_dir && expanded.contains(&node.path) && node.children.is_none() {
            node.expanded = true;
            node.children = Some(match list_children(&node.path) {
                Ok(c) => c,
                Err(_) => {
                    node.unreadable = true;
                    Vec::new()
                }
            });
        }
        if let Some(children) = &mut node.children {
            reexpand(children, expanded);
        }
    }
}

fn sort_entries(nodes: &mut [Node]) {
    nodes.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// Lists one directory (no recursion). Symlinked directories are treated
/// as files so the tree can never cycle; unreadable dirs are marked.
fn list_children(dir: &Path) -> Result<Vec<Node>, std::io::Error> {
    let mut nodes = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue, // vanished mid-listing; skip
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        // Don't follow symlinked directories (infinite loops).
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        nodes.push(Node {
            path,
            name,
            is_dir,
            expanded: false,
            children: None,
            unreadable: false,
        });
    }
    sort_entries(&mut nodes);
    Ok(nodes)
}

impl Workspace {
    /// Opens a folder as a workspace, listing its top level only.
    pub fn open(root: &Path) -> Result<Self, std::io::Error> {
        let nodes = list_children(root)?;
        Ok(Self {
            root: root.to_path_buf(),
            nodes,
            selected: None,
        })
    }

    /// The folder name, for display.
    pub fn title(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string())
    }

    /// Loads the children of a directory node the first time it expands.
    pub fn expand(&mut self, path: &Path) {
        let Some(node) = self.find_deep_mut(path) else {
            return;
        };
        if !node.is_dir || node.children.is_some() || node.unreadable {
            return;
        }
        node.children = Some(match list_children(path) {
            Ok(children) => children,
            Err(_) => {
                node.unreadable = true;
                Vec::new()
            }
        });
    }

    /// Reloads the tree (the Refresh action), preserving which folders
    /// were expanded so the user's place isn't lost.
    pub fn refresh(&mut self) {
        let mut expanded = std::collections::HashSet::new();
        collect_expanded(&self.nodes, &mut expanded);

        self.nodes = match list_children(&self.root) {
            Ok(nodes) => nodes,
            Err(_) => {
                for node in &mut self.nodes {
                    node.unreadable = true;
                }
                return;
            }
        };

        reexpand(&mut self.nodes, &expanded);

        // Drop the selection if it no longer exists.
        if self
            .selected
            .as_ref()
            .is_some_and(|s| self.find(s).is_none())
        {
            self.selected = None;
        }
    }

    fn find(&self, path: &Path) -> Option<&Node> {
        self.nodes.iter().find(|n| n.path == path)
    }

    /// Finds a node at any depth.
    pub fn find_deep(&self, path: &Path) -> Option<&Node> {
        find_deep(&self.nodes, path)
    }

    /// Mutable variant of [`Self::find_deep`].
    pub fn find_deep_mut(&mut self, path: &Path) -> Option<&mut Node> {
        find_deep_mut(&mut self.nodes, path)
    }

    /// Creates a new file or directory inside `dir`.
    pub fn create(dir: &Path, name: &str, is_dir: bool) -> std::io::Result<PathBuf> {
        let path = dir.join(name);
        if is_dir {
            std::fs::create_dir(&path)?;
        } else {
            std::fs::File::create(&path)?;
        }
        Ok(path)
    }

    /// Renames a file or directory (same parent directory).
    pub fn rename(path: &Path, new_name: &str) -> std::io::Result<PathBuf> {
        let dir = path.parent().ok_or_else(|| io_err("no parent directory"))?;
        let new_path = dir.join(new_name);
        std::fs::rename(path, &new_path)?;
        Ok(new_path)
    }

    /// Deletes a *file*. Directories are not deleted (too easy to destroy).
    pub fn delete_file(path: &Path) -> std::io::Result<()> {
        std::fs::remove_file(path)
    }

    /// Re-reads `dir` and splices the fresh entries into the tree
    /// (top level if it is the root, otherwise into the matching parent).
    pub fn relist(&mut self, dir: &Path) {
        let entries = match list_children(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        if dir == self.root {
            self.nodes = entries;
        } else if let Some(parent) = self.find_deep_mut(dir) {
            parent.children = Some(entries);
        }
    }
}

fn find_deep<'a>(nodes: &'a [Node], path: &Path) -> Option<&'a Node> {
    for node in nodes {
        if node.path == path {
            return Some(node);
        }
        if let Some(children) = &node.children
            && let Some(found) = find_deep(children, path)
        {
            return Some(found);
        }
    }
    None
}

fn find_deep_mut<'a>(nodes: &'a mut [Node], path: &Path) -> Option<&'a mut Node> {
    for node in nodes.iter_mut() {
        if node.path == path {
            return Some(node);
        }
        if let Some(children) = &mut node.children
            && let Some(found) = find_deep_mut(children, path)
        {
            return Some(found);
        }
    }
    None
}

fn io_err(msg: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::NotFound, msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("b.py"), "").unwrap();
        fs::write(dir.path().join("a.java"), "").unwrap();
        fs::create_dir(dir.path().join("zeta")).unwrap();
        fs::create_dir(dir.path().join("alpha")).unwrap();
        dir
    }

    #[test]
    fn opens_and_sorts_dirs_first() {
        let dir = setup();
        let ws = Workspace::open(dir.path()).unwrap();
        let names: Vec<&str> = ws.nodes.iter().map(|n| n.name.as_str()).collect();
        // Directories first, then case-insensitive alpha.
        assert_eq!(names, vec!["alpha", "zeta", "a.java", "b.py"]);
        assert!(ws.nodes[0].is_dir && !ws.nodes[0].expanded);
        assert!(ws.nodes[0].children.is_none()); // lazy
    }

    #[test]
    fn expand_loads_children_lazily() {
        let dir = setup();
        fs::write(dir.path().join("alpha/inner.c"), "").unwrap();
        let mut ws = Workspace::open(dir.path()).unwrap();
        let alpha = dir.path().join("alpha");
        ws.expand(&alpha);
        let node = ws.find_deep(&alpha).unwrap();
        assert!(node.children.is_some());
        assert_eq!(node.children.as_ref().unwrap().len(), 1);
        // Expanding again is a no-op.
        ws.expand(&alpha);
        assert_eq!(
            ws.find_deep(&alpha)
                .unwrap()
                .children
                .as_ref()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn create_rename_delete_round_trip() {
        let dir = setup();
        let path = Workspace::create(dir.path(), "new.py", false).unwrap();
        assert!(path.exists());

        let renamed = Workspace::rename(&path, "renamed.py").unwrap();
        assert!(renamed.exists() && !path.exists());

        Workspace::delete_file(&renamed).unwrap();
        assert!(!renamed.exists());
    }

    #[test]
    fn missing_root_is_an_error() {
        assert!(Workspace::open(Path::new("/nonexistent/rustcode/ws")).is_err());
    }

    #[test]
    fn refresh_preserves_expansion() {
        let dir = setup();
        let alpha = dir.path().join("alpha");
        fs::write(alpha.join("inner.c"), "").unwrap();
        let mut ws = Workspace::open(dir.path()).unwrap();
        // Mirror the UI: toggle expanded and ensure children are loaded.
        if let Some(node) = ws.find_deep_mut(&alpha) {
            node.expanded = true;
        }
        ws.expand(&alpha);
        // Add a new sibling while "away", then refresh.
        fs::write(alpha.join("later.py"), "").unwrap();

        ws.refresh();
        let a = ws.find_deep(&alpha).unwrap();
        assert!(a.expanded, "expanded state survives refresh");
        let names: Vec<&str> = a
            .children
            .as_ref()
            .unwrap()
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert!(names.contains(&"inner.c") && names.contains(&"later.py"));
    }

    #[test]
    fn symlinks_are_not_directories() {
        let dir = setup();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("alpha"), dir.path().join("loop")).unwrap();
        }
        let ws = Workspace::open(dir.path()).unwrap();
        let loop_node = ws.nodes.iter().find(|n| n.name == "loop");
        // Symlinked dirs show up as non-directories (no expansion, no cycles).
        assert!(loop_node.is_some_and(|n| !n.is_dir));
    }
}
