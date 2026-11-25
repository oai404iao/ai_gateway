WITH eligible AS (
    SELECT channel_group.id AS channel_group_id, log.api_format,
           COALESCE(log.upstream_model, log.client_model) AS model,
           to_timestamp(floor(extract(epoch FROM log.started_at) / $3::double precision)
                        * $3::double precision) AS bucket_started_at,
           log.outcome, log.ttft_ms, log.output_tokens_per_second
    FROM request_logs AS log
    JOIN group_identity_registry AS group_identity ON group_identity.id = log.channel_group_id
    JOIN routing_groups AS channel_group ON channel_group.id = group_identity.canonical_group_id
    JOIN channel_identity_registry AS identity ON identity.id = log.channel_id
    JOIN upstream_channels AS channel ON channel.id = identity.canonical_channel_id
    JOIN channel_capabilities AS capability ON capability.channel_id = channel.id
                                          AND capability.operation = log.api_operation
    WHERE capability.status_statistics_enabled AND capability.deleted_at IS NULL
      AND channel.deleted_at IS NULL AND channel_group.deleted_at IS NULL
      AND log.started_at >= $1 AND log.started_at < $2
)
SELECT channel_group_id, api_format::text AS api_format, model, bucket_started_at,
       count(*) AS request_count,
       count(*) FILTER (WHERE outcome <> 'cancelled') AS success_rate_request_count,
       count(*) FILTER (WHERE outcome = 'succeeded') AS succeeded_count,
       percentile_cont(0.9) WITHIN GROUP (ORDER BY ttft_ms::double precision)
           FILTER (WHERE outcome = 'succeeded' AND ttft_ms IS NOT NULL) AS p90_ttft_ms,
       percentile_cont(0.5) WITHIN GROUP (ORDER BY output_tokens_per_second::double precision)
           FILTER (WHERE outcome = 'succeeded' AND output_tokens_per_second IS NOT NULL) AS p50_tps
FROM eligible
GROUP BY api_format, model, ROLLUP(channel_group_id, bucket_started_at)
