import type { Project } from '@/api'

export interface TreeNode {
  name: string
  fullPath: string
  type: 'folder' | 'project'
  children: Map<string, TreeNode>
  project?: Project // Only if type is 'project'
  count: number // Total projects inside
}

export function buildProjectTree(projects: Project[]): TreeNode {
  const root: TreeNode = {
    name: 'root',
    fullPath: '',
    type: 'folder',
    children: new Map(),
    count: 0
  }

  for (const project of projects) {
    // Normalize path: remove leading slash, split by /
    const parts = project.path.split('/').filter(Boolean)
    let currentNode = root
    root.count++

    for (let i = 0; i < parts.length; i++) {
      const part = parts[i]
      if (!part) continue

      const isLast = i === parts.length - 1

      if (!currentNode.children.has(part)) {
        const newNode: TreeNode = {
          name: part,
          fullPath: parts.slice(0, i + 1).join('/'),
          type: isLast ? 'project' : 'folder',
          children: new Map(),
          count: 0
        }
        currentNode.children.set(part, newNode)
      }

      const nextNode = currentNode.children.get(part)
      if (nextNode) {
        currentNode = nextNode
        currentNode.count++

        // If we are traversing through a node that was previously marked as a project,
        // it must now become a folder because it has children.
        if (!isLast && currentNode.type === 'project') {
          currentNode.type = 'folder'
        }
      }

      if (isLast) {
        currentNode.project = project
        // Only mark as project type if it has no children (leaf).
        // If it has children, it stays 'folder' but now has .project data.
        if (currentNode.children.size === 0) {
          currentNode.type = 'project'
        }
      }
    }
  }

  // Compact the tree
  compactNode(root)

  return root
}

function compactNode(node: TreeNode) {
  // First, compact all children recursively
  for (const child of node.children.values()) {
    compactNode(child)
  }

  // Now check if we can compact this node
  // We don't compact the root node itself (name === 'root')
  if (node.name === 'root') return

  // If this node is a folder (not a project itself) and has exactly one child
  // UPDATE: If it IS a project (hybrid), we should NOT compact it, 
  // because hiding it would hide the project data.
  if (!node.project && node.children.size === 1) {
    const child = node.children.values().next().value!

    // Merge child into current node
    node.name = `${node.name}/${child.name}`
    node.fullPath = child.fullPath
    node.type = child.type
    node.project = child.project
    node.children = child.children
    node.count = child.count

    // Since we merged, the current node effectively became the child (plus name prefix).
    // The child might have been further compactable, but we already compacted children first (post-order),
    // so the child is already maximally compacted.
    // However, by merging, we brought the child's children up.
    // We don't need to re-check because we are just extending the path.
  }
}

// Helper to get nodes at a specific path
export function getNodesAtLevel(root: TreeNode, path: string[]): TreeNode[] {
  let current = root
  for (const part of path) {
    if (current.children.has(part)) {
      current = current.children.get(part)!
    } else {
      return []
    }
  }
  return Array.from(current.children.values()).sort((a, b) => {
    // Folders first, then projects
    if (a.type !== b.type) {
      return a.type === 'folder' ? -1 : 1
    }
    return a.name.localeCompare(b.name)
  })
}
