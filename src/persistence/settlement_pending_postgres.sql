SELECT log.id
FROM request_settlement_pending AS pending
JOIN request_metering_facts AS log ON log.id=pending.request_id
JOIN api_keys AS key
  ON key.id = log.api_key_id
 AND key.user_id = log.user_id
WHERE log.amount_state IN ('priced','zero_by_policy')
ORDER BY pending.completed_at, pending.request_id
LIMIT $1
