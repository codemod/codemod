-- Third-party packages whose react or react-dom range refuses 19.0.0.
SELECT f.name || '@' || f.version AS package, group_concat(e.name || ' ' || e.req, '; ') AS refuses
FROM edges e JOIN nodes f ON f.id = e.src
WHERE f.workspace IS NULL
  AND e.name IN ('react', 'react-dom') AND e.kind IN ('peer', 'prod', 'optional')
  AND satisfies(f.eco, e.req, '19.0.0') = 0
  AND f.name NOT IN ('react', 'react-dom', 'react-test-renderer', 'react-reconciler')
  AND f.name NOT LIKE '@yarnpkg/%' AND f.name NOT LIKE 'ink-%'
GROUP BY f.id ORDER BY package;
