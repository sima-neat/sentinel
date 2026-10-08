//! Device-tree reading for the `board` block: a bounded, read-only parser for
//! flattened device trees (overlay `.dtbo` files), a bounded reader for a
//! subtree of the live tree under `/sys/firmware/devicetree/base`, and the
//! rule that finds the MIPI CSI-2 sources in either.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use super::super::sysutil::bounded_string;

/// Deeper nesting is malformed (FDT) or not read (live tree).
const MAX_DEPTH: usize = 32;
/// Nodes read from one live subtree; the rest are not read.
const MAX_LIVE_NODES: usize = 256;
/// Entries listed in one live node directory.
const MAX_LIVE_ENTRIES: usize = 256;
/// A live property is read up to this size.
pub(super) const MAX_PROPERTY_BYTES: u64 = 4096;

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

/// One node; `parent` always comes before it, so the nodes are in preorder.
pub(super) struct Node {
    pub(super) name: String,
    pub(super) parent: Option<usize>,
    pub(super) properties: Vec<(String, Vec<u8>)>,
}

/// A device tree, or a subtree of one; node 0 is its root.
#[derive(Default)]
pub(super) struct Tree {
    pub(super) nodes: Vec<Node>,
}

impl Tree {
    fn push(&mut self, name: String, parent: Option<usize>) -> usize {
        self.nodes.push(Node {
            name,
            parent,
            properties: Vec::new(),
        });
        self.nodes.len() - 1
    }

    fn property(&self, node: usize, name: &str) -> Option<&[u8]> {
        let properties = &self.nodes[node].properties;
        let found = properties.iter().find(|(key, _)| key == name);
        found.map(|(_, value)| value.as_slice())
    }

    /// The first string of the node's `compatible` property.
    pub(super) fn compatible(&self, node: usize) -> Option<String> {
        let value = self.property(node, "compatible")?;
        let first = value
            .split(|&byte| byte == 0)
            .find(|part| !part.is_empty())?;
        Some(String::from_utf8_lossy(first).into_owned())
    }
}

/// Every node that is a MIPI CSI-2 source, with its data-lane count, in
/// preorder. A node is a source when it has `compatible` (it is a device) and
/// an `endpoint` node below it, not inside another device, has `data-lanes`;
/// the first such endpoint in preorder gives the count (the number of cells).
/// A source with another source below it is a bridge (an I2C mux, a GMSL
/// deserializer or serializer) and is left out, so only the innermost source,
/// the sensor, remains.
pub(super) fn csi2_sources(tree: &Tree) -> Vec<(usize, u32)> {
    let count = tree.nodes.len();
    // Lanes of the first endpoint below each node, through non-device children.
    let mut below = vec![None::<u32>; count];
    let mut lanes = vec![None::<u32>; count];
    let mut source = vec![false; count];
    // Whether a source is strictly below the node.
    let mut inner = vec![false; count];
    // Descendants come after their node and earlier siblings before later
    // ones, so in reverse each node sees its finished subtree and the first
    // child overwrites the later ones.
    for index in (0..count).rev() {
        let node = &tree.nodes[index];
        let base_name = node.name.split('@').next().unwrap_or_default();
        let own = (base_name == "endpoint")
            .then(|| tree.property(index, "data-lanes"))
            .flatten()
            .map(|value| (value.len() / 4) as u32);
        lanes[index] = own.or(below[index]);
        let device = tree.property(index, "compatible").is_some();
        source[index] = device && lanes[index].is_some();
        if let Some(parent) = node.parent {
            if !device && lanes[index].is_some() {
                below[parent] = lanes[index];
            }
            inner[parent] |= source[index] || inner[index];
        }
    }
    let reported = (0..count).filter(|&index| source[index] && !inner[index]);
    reported
        .filter_map(|index| Some((index, lanes[index]?)))
        .collect()
}

/// The sorted, distinct `compatible` strings of the CSI-2 sources an overlay
/// adds to an I2C bus: below a node whose name starts with `i2c`, or in a
/// fragment whose target is one, by `target-path` or by the label that
/// `__fixups__` resolves its `target` phandle to.
pub(super) fn overlay_sensors(tree: &Tree) -> Vec<String> {
    let targets = fixup_targets(tree);
    let on_i2c_bus = |index: usize| {
        let mut ancestor = tree.nodes[index].parent;
        while let Some(current) = ancestor {
            let node = &tree.nodes[current];
            if node.name.starts_with("i2c") {
                return true;
            }
            if node.parent == Some(0) {
                let path = tree.property(current, "target-path").map(bounded_string);
                let last = path.as_deref().and_then(|path| path.rsplit('/').next());
                let label = targets.get(&node.name).map(String::as_str);
                return last.is_some_and(|name| name.starts_with("i2c"))
                    || label.is_some_and(|label| label.starts_with("i2c"));
            }
            ancestor = node.parent;
        }
        false
    };
    let sensors = csi2_sources(tree).into_iter().map(|(index, _)| index);
    let sensors = sensors.filter(|&index| on_i2c_bus(index));
    let names: BTreeSet<_> = sensors.filter_map(|index| tree.compatible(index)).collect();
    names.into_iter().collect()
}

/// Top-level fragment name to the label its `target` phandle refers to, from
/// the overlay's `__fixups__` node (`label = "/fragment@7:target:0"`).
fn fixup_targets(tree: &Tree) -> BTreeMap<String, String> {
    let mut targets = BTreeMap::new();
    let fixups = (1..tree.nodes.len()).find(|&index| {
        tree.nodes[index].parent == Some(0) && tree.nodes[index].name == "__fixups__"
    });
    let Some(fixups) = fixups else {
        return targets;
    };
    for (label, value) in &tree.nodes[fixups].properties {
        for entry in value.split(|&byte| byte == 0) {
            let entry = String::from_utf8_lossy(entry);
            let mut parts = entry.rsplitn(3, ':');
            let (_offset, property, path) = (parts.next(), parts.next(), parts.next());
            let fragment = path.and_then(|path| path.strip_prefix('/'));
            if let (Some("target"), Some(fragment)) = (property, fragment) {
                targets.insert(fragment.to_string(), label.clone());
            }
        }
    }
    targets
}

/// Parse a flattened device tree (big-endian, version 16 or later). Every
/// offset and length is checked against the blob; the reason names the first
/// problem.
pub(super) fn parse_fdt(data: &[u8]) -> Result<Tree, String> {
    let word = |bytes: &[u8], offset: usize| -> Result<u32, String> {
        let end = offset.checked_add(4).filter(|&end| end <= bytes.len());
        let field = end.map(|end| &bytes[offset..end]);
        let field = field.ok_or_else(|| format!("truncated at byte {offset}"))?;
        Ok(u32::from_be_bytes([field[0], field[1], field[2], field[3]]))
    };
    if word(data, 0)? != FDT_MAGIC {
        return Err("not a flattened device tree (bad magic)".into());
    }
    let total = word(data, 4)? as usize;
    if total > data.len() {
        return Err(format!(
            "truncated: the header says {total} bytes, the file has {}",
            data.len()
        ));
    }
    let data = &data[..total];
    let version = word(data, 20)?;
    if version < 16 {
        return Err(format!("unsupported version {version}"));
    }
    let block = |offset: u32, size: usize| {
        let start = offset as usize;
        let end = start.checked_add(size).filter(|&end| end <= data.len());
        end.map(|end| &data[start..end])
    };
    let struct_offset = word(data, 8)?;
    let struct_size = match version {
        16 => total.saturating_sub(struct_offset as usize),
        _ => word(data, 36)? as usize,
    };
    let structure = block(struct_offset, struct_size).ok_or("structure block out of range")?;
    let strings = block(word(data, 12)?, word(data, 32)? as usize);
    let strings = strings.ok_or("strings block out of range")?;
    // The NUL-terminated string at `offset`, without its NUL.
    let text = |bytes: &[u8], offset: usize| -> Result<Vec<u8>, String> {
        let rest = bytes.get(offset..).unwrap_or_default();
        let end = rest.iter().position(|&byte| byte == 0);
        let end = end.ok_or_else(|| format!("unterminated string at byte {offset}"))?;
        Ok(rest[..end].to_vec())
    };
    let lossy = |bytes: Vec<u8>| String::from_utf8_lossy(&bytes).into_owned();
    let align = |offset: usize| (offset + 3) & !3;
    let mut tree = Tree::default();
    let mut open: Vec<usize> = Vec::new();
    let mut position = 0;
    loop {
        let token = word(structure, position)?;
        position += 4;
        match token {
            FDT_BEGIN_NODE => {
                let name = text(structure, position)?;
                position = align(position + name.len() + 1);
                let name = lossy(name);
                if open.len() >= MAX_DEPTH {
                    return Err(format!("nodes nested deeper than {MAX_DEPTH}"));
                }
                if open.is_empty() && !tree.nodes.is_empty() {
                    return Err("more than one root node".into());
                }
                let node = tree.push(name, open.last().copied());
                open.push(node);
            }
            FDT_END_NODE => {
                open.pop().ok_or("unbalanced end of node")?;
            }
            FDT_PROP => {
                let length = word(structure, position)? as usize;
                let name = lossy(text(strings, word(structure, position + 4)? as usize)?);
                let start = position + 8;
                let value = start
                    .checked_add(length)
                    .and_then(|end| structure.get(start..end));
                let value = value.ok_or_else(|| format!("property {name} out of range"))?;
                position = align(start + length);
                let node = *open.last().ok_or("property outside a node")?;
                tree.nodes[node].properties.push((name, value.to_vec()));
            }
            FDT_NOP => {}
            FDT_END if open.is_empty() && !tree.nodes.is_empty() => return Ok(tree),
            FDT_END => return Err("ended inside a node or before the root".into()),
            other => return Err(format!("unknown token {other:#x} at byte {position}")),
        }
    }
}

/// At most `limit` bytes of the file at `path`.
pub(super) fn read_bounded(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// At most `limit` entry names of `directory`, sorted, and whether it has
/// more. Which entries are left out of a truncated listing is arbitrary.
pub(super) fn listed(directory: &Path, limit: usize) -> io::Result<(Vec<String>, bool)> {
    let mut names = Vec::new();
    let mut entries = fs::read_dir(directory)?;
    let mut truncated = false;
    for entry in entries.by_ref() {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if names.len() == limit {
            truncated = true;
            break;
        }
        names.push(name);
    }
    names.sort();
    Ok((names, truncated))
}

/// Why `listed` left entries of `directory` out.
pub(super) fn too_many(limit: usize, what: &str, directory: &Path) -> String {
    format!(
        "more than {limit} {what} in {}; the rest were not read",
        directory.display()
    )
}

/// The live subtree at `path` (a node directory under
/// `/sys/firmware/devicetree/base`), with the `compatible` and `data-lanes`
/// properties only. Bounded in depth, entries per node, and nodes; the second
/// value says why part of the subtree was not read, when a bound was hit.
pub(super) fn read_live(path: &Path) -> io::Result<(Tree, Option<String>)> {
    let mut tree = Tree::default();
    let mut truncated = None;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    // Children are pushed in reverse so they are visited in sorted order.
    let mut pending: Vec<(PathBuf, String, Option<usize>, usize)> =
        vec![(path.to_path_buf(), name.unwrap_or_default(), None, 0)];
    while let Some((directory, name, parent, depth)) = pending.pop() {
        if tree.nodes.len() >= MAX_LIVE_NODES {
            let reason = too_many(MAX_LIVE_NODES, "device-tree nodes", path);
            truncated.get_or_insert(reason);
            break;
        }
        let node = tree.push(name, parent);
        let mut children = Vec::new();
        let (entries, more) = listed(&directory, MAX_LIVE_ENTRIES)?;
        if more {
            truncated.get_or_insert_with(|| too_many(MAX_LIVE_ENTRIES, "entries", &directory));
        }
        for entry in entries {
            let entry_path = directory.join(&entry);
            let metadata = match fs::symlink_metadata(&entry_path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if metadata.is_dir() {
                if depth + 1 < MAX_DEPTH {
                    children.push((entry_path, entry, Some(node), depth + 1));
                } else {
                    truncated.get_or_insert_with(|| {
                        let path = entry_path.display();
                        format!("{path} is nested deeper than {MAX_DEPTH} nodes and was not read")
                    });
                }
            } else if entry == "compatible" || entry == "data-lanes" {
                match read_bounded(&entry_path, MAX_PROPERTY_BYTES) {
                    Ok(value) => tree.nodes[node].properties.push((entry, value)),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
        pending.extend(children.into_iter().rev());
    }
    Ok((tree, truncated))
}
