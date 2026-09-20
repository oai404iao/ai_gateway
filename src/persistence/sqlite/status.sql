WITH base AS MATERIALIZED (
    -- Date math rounds fractional seconds; bucket identity must truncate them instead.
    SELECT log.channel_group_id,log.api_format,
           COALESCE(log.upstream_model,log.client_model) AS model,log.outcome,
           log.ttft_ms,log.output_tokens_per_second,
           unixepoch(substr(log.started_at,1,19)||'Z') -
             ((unixepoch(substr(log.started_at,1,19)||'Z') % ?3 + ?3) % ?3) AS bucket
    FROM request_logs AS log
    JOIN channel_groups AS g ON g.id=log.channel_group_id
    WHERE g.status_statistics_enabled AND g.deleted_at IS NULL
      AND log.started_at >= ?1 AND log.started_at < ?2
), scoped AS MATERIALIZED (
    SELECT scope,CASE WHEN scope=0 THEN NULL ELSE channel_group_id END AS group_id,
           api_format,model,CASE WHEN scope=2 THEN bucket ELSE 0 END AS bucket,
           outcome,ttft_ms,output_tokens_per_second
    FROM base CROSS JOIN (SELECT 0 AS scope UNION ALL SELECT 1 UNION ALL SELECT 2)
), counts AS (
    SELECT scope,group_id,api_format,model,bucket,count(*) AS request_count,
           sum(outcome!='cancelled') AS success_rate_request_count,
           sum(outcome='succeeded') AS succeeded_count
    FROM scoped GROUP BY scope,group_id,api_format,model,bucket
), samples AS (
    SELECT scope,group_id,api_format,model,bucket,0 AS metric,CAST(ttft_ms AS REAL) AS value
    FROM scoped WHERE outcome='succeeded' AND ttft_ms IS NOT NULL
    UNION ALL
    SELECT scope,group_id,api_format,model,bucket,1,CAST(output_tokens_per_second AS REAL)
    FROM scoped WHERE outcome='succeeded' AND output_tokens_per_second IS NOT NULL
), ordered AS (
    SELECT *,row_number() OVER (PARTITION BY scope,group_id,api_format,model,bucket,metric ORDER BY value)-1 AS position,
           (count(*) OVER (PARTITION BY scope,group_id,api_format,model,bucket,metric)-1) *
              CASE WHEN metric=0 THEN 0.9 ELSE 0.5 END AS target
    FROM samples
), bounds AS (
    SELECT scope,group_id,api_format,model,bucket,metric,target,
           max(CASE WHEN position=CAST(target AS INTEGER) THEN value END) AS lo,
           max(CASE WHEN position=CAST(target AS INTEGER)+(target>CAST(target AS INTEGER)) THEN value END) AS hi
    FROM ordered
    WHERE position=CAST(target AS INTEGER)
       OR position=CAST(target AS INTEGER)+(target>CAST(target AS INTEGER))
    GROUP BY scope,group_id,api_format,model,bucket,metric
), percentiles AS (
    SELECT scope,group_id,api_format,model,bucket,
           max(CASE WHEN metric=0 THEN lo+(hi-lo)*(target-CAST(target AS INTEGER)) END) AS p90_ttft_ms,
           max(CASE WHEN metric=1 THEN lo+(hi-lo)*(target-CAST(target AS INTEGER)) END) AS p50_tps
    FROM bounds GROUP BY scope,group_id,api_format,model,bucket
)
SELECT c.*,p.p90_ttft_ms,p.p50_tps FROM counts c LEFT JOIN percentiles p
 ON c.scope=p.scope AND c.group_id IS p.group_id AND c.api_format=p.api_format
 AND c.model=p.model AND c.bucket=p.bucket
