ALTER TABLE proxies
    DROP CONSTRAINT proxies_proxy_url_check,
    ADD CONSTRAINT proxies_proxy_url_check
        CHECK (proxy_url ~* '^(https?|socks4a?|socks5h?)://');
