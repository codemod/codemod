-- Keep legacy_users in comments and string literals.
SELECT id, 'legacy_users' AS label FROM users WHERE active = true;
SELECT id FROM public.users;
SELECT legacy_users AS legacy_users FROM legacy_users.users;
