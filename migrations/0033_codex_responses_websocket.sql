UPDATE channels AS channel
SET supports_websocket = true
FROM channel_groups AS channel_group
WHERE channel.channel_group_id = channel_group.id
  AND channel_group.connector_kind = 'codex_oauth'
  AND NOT channel.supports_websocket;
