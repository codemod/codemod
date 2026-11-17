-- Who reaches react-beautiful-dnd, and in how many hops (1 = direct).
-- Walks up from the target; a walk stops at a workspace, so each workspace answers for itself.
WITH RECURSIVE up(node, hops) AS (
  SELECT id, 0 FROM nodes WHERE name = 'react-beautiful-dnd'
  UNION
  SELECT e.src, up.hops + 1
  FROM up JOIN edges e ON e.dst = up.node JOIN nodes n ON n.id = up.node
  WHERE (up.hops = 0 OR n.workspace IS NULL) AND e.kind <> 'peer' AND up.hops < 12
)
SELECT n.name AS workspace, n.workspace AS dir, min(hops) AS hops
FROM up JOIN nodes n ON n.id = up.node
WHERE n.workspace IS NOT NULL AND up.hops > 0
GROUP BY n.id ORDER BY hops, workspace;
