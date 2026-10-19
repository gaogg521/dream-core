UPDATE users SET sso_only = 1,
    sso_provider = (SELECT provider FROM one_sso_identities s WHERE s.user_id = users.id ORDER BY last_seen_at DESC, provider ASC LIMIT 1),
    must_change_password = 0
WHERE EXISTS (SELECT 1 FROM one_sso_identities s WHERE s.user_id = users.id);
