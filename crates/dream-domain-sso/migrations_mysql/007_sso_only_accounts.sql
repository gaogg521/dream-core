UPDATE users u JOIN (
    SELECT user_id, MIN(provider) AS provider FROM one_sso_identities GROUP BY user_id
) s ON s.user_id = u.id
SET u.sso_only = TRUE, u.sso_provider = s.provider;
