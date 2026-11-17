-- The bump plan: each blocker, the first release that accepts React 19 ('none' = stuck),
-- and how many workspaces it reaches directly vs transitively.
WITH RECURSIVE blockers AS (
  SELECT DISTINCT f.id, f.eco, f.name, f.version
  FROM edges e JOIN nodes f ON f.id = e.src
  WHERE f.workspace IS NULL AND e.name = 'react' AND e.kind IN ('peer', 'prod')
    AND satisfies(f.eco, e.req, '19.0.0') = 0
    AND f.name NOT IN ('react-dom', 'react-test-renderer', 'react-reconciler')
    AND f.name NOT LIKE '@yarnpkg/%' AND f.name NOT LIKE 'ink-%'
),
up(blocker, node, hops) AS (
  SELECT id, id, 0 FROM blockers
  UNION
  SELECT up.blocker, e.src, up.hops + 1
  FROM up JOIN edges e ON e.dst = up.node JOIN nodes n ON n.id = up.node
  WHERE (up.hops = 0 OR n.workspace IS NULL) AND e.kind <> 'peer' AND up.hops < 12
),
reach AS (
  SELECT up.blocker, up.node, min(up.hops) AS hops FROM up JOIN nodes w ON w.id = up.node
  WHERE w.workspace IS NOT NULL GROUP BY up.blocker, up.node
)
SELECT b.name || '@' || b.version AS blocker,
       first_release_accepting(b.eco, b.name, b.version, 'react', '19.0.0') AS fix,
       sum(r.hops = 1) AS direct, sum(r.hops > 1) AS transitive
FROM blockers b LEFT JOIN reach r ON r.blocker = b.id
GROUP BY b.id ORDER BY fix = 'none' DESC, direct DESC;
