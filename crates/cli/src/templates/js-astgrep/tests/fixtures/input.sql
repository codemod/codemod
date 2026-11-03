-- Keep legacy_users in comments and string literals.
SELECT id, 'legacy_users' AS label FROM legacy_users WHERE active = true;
SELECT id FROM public.legacy_users;
SELECT legacy_users AS legacy_users FROM legacy_users.legacy_users;
