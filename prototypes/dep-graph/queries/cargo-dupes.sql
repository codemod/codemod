-- Crates locked in more than one version, and which crate pulls each version.
SELECT n.name, n.version, group_concat(DISTINCT f.name) AS pulled_by
FROM nodes n JOIN edges e ON e.dst = n.id JOIN nodes f ON f.id = e.src
WHERE n.eco = 'cargo' AND n.name IN (
  SELECT name FROM nodes WHERE eco = 'cargo' AND workspace IS NULL GROUP BY name HAVING count(*) > 1)
GROUP BY n.id ORDER BY n.name, n.version;
