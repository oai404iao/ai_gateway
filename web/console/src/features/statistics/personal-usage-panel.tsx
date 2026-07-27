import { useMemo } from "react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { AsyncResource } from "@/components/shared/async-resource";
import { usePersonalUsage } from "@/features/statistics/api";
import type { PersonalUsageDay } from "@/api/types";
import { cn } from "@/lib/utils";
import { useI18n, type ConsoleLocale } from "@/app/i18n";

const INTENSITY_CLASSES = [
  "bg-muted",
  "bg-success/20",
  "bg-success/40",
  "bg-success/70",
  "bg-success",
] as const;

interface UsageWeek {
  days: Array<PersonalUsageDay | null>;
}

interface MonthLabel {
  weekIndex: number;
  weekSpan: number;
  label: string;
}

function parseUtcDate(value: string): Date {
  const [year, month, day] = value.split("-").map(Number);
  return new Date(Date.UTC(year, month - 1, day));
}

function formatUtcDate(value: string, locale: ConsoleLocale): string {
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeZone: "UTC",
  }).format(parseUtcDate(value));
}

function buildWeeks(days: PersonalUsageDay[]): UsageWeek[] {
  if (days.length === 0) return [];
  const leadingEmptyDays = parseUtcDate(days[0].date).getUTCDay();
  const cells: Array<PersonalUsageDay | null> = [
    ...Array.from({ length: leadingEmptyDays }, () => null),
    ...days,
  ];
  while (cells.length % 7 !== 0) {
    cells.push(null);
  }
  return Array.from({ length: cells.length / 7 }, (_, index) => ({
    days: cells.slice(index * 7, index * 7 + 7),
  }));
}

function buildMonthLabels(
  weeks: UsageWeek[],
  locale: ConsoleLocale,
): MonthLabel[] {
  const markers = weeks.flatMap((week, weekIndex) => {
    const firstDay = week.days.find((day): day is PersonalUsageDay => day !== null);
    const firstOfMonth = week.days.find(
      (day): day is PersonalUsageDay => day?.date.endsWith("-01") ?? false,
    );
    const marker = firstOfMonth ?? (weekIndex === 0 ? firstDay : null);
    if (!marker) return [];
    return [
      {
        weekIndex,
        label: new Intl.DateTimeFormat(locale, {
          month: "short",
          timeZone: "UTC",
        }).format(parseUtcDate(marker.date)),
      },
    ];
  });

  return markers.map((marker, index) => ({
    ...marker,
    weekSpan:
      (markers[index + 1]?.weekIndex ?? weeks.length) - marker.weekIndex,
  }));
}

function intensityLevel(requestCount: number, maximum: number): number {
  if (requestCount <= 0 || maximum <= 0) return 0;
  return Math.max(
    1,
    Math.min(
      4,
      Math.ceil(
        (Math.log(requestCount + 1) / Math.log(maximum + 1)) * 4,
      ),
    ),
  );
}

function MetricCard({
  title,
  value,
  description,
}: {
  title: string;
  value: React.ReactNode;
  description: string;
}) {
  return (
    <Card size="sm">
      <CardHeader>
        <CardDescription>{title}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-1">
        <CardTitle className="text-2xl tabular-nums">{value}</CardTitle>
        <CardDescription className="text-xs">{description}</CardDescription>
      </CardContent>
    </Card>
  );
}

export function PersonalUsagePanel() {
  const query = usePersonalUsage();
  const { locale, t } = useI18n();
  const data = query.data;
  const calendar = useMemo(() => {
    const weeks = buildWeeks(data?.days ?? []);
    return {
      weeks,
      monthLabels: buildMonthLabels(weeks, locale),
      maximum: Math.max(
        0,
        ...(data?.days.map((day) => day.request_count) ?? []),
      ),
    };
  }, [data, locale]);

  return (
    <AsyncResource isLoading={query.isLoading} error={query.error}>
      {data ? (
        <div className="flex flex-col gap-6">
          <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
            <MetricCard
              title={t("Requests")}
              value={data.total_request_count.toLocaleString(locale)}
              description={t("Client requests in the 365-day window.")}
            />
            <MetricCard
              title={t("Active days")}
              value={`${data.active_day_count.toLocaleString(locale)} / 365`}
              description={t("UTC dates with at least one client request.")}
            />
            <MetricCard
              title={t("Current streak")}
              value={t("{count} days", {
                count: data.current_streak_days.toLocaleString(locale),
              })}
              description={t(
                "Consecutive active UTC dates through the current day.",
              )}
            />
            <MetricCard
              title={t("Longest streak")}
              value={t("{count} days", {
                count: data.longest_streak_days.toLocaleString(locale),
              })}
              description={t("Longest active-day run in this window.")}
            />
          </div>

          <Card>
            <CardHeader>
              <CardTitle>{t("Request activity")}</CardTitle>
              <CardDescription>
                {t(
                  "{start} – {end} · UTC dates; scheduled tests are excluded.",
                  {
                    start: formatUtcDate(data.started_on, locale),
                    end: formatUtcDate(data.ended_on, locale),
                  },
                )}
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              <div className="overflow-x-auto pb-2">
                <div className="min-w-max">
                  <div
                    className="ml-9 grid h-5 gap-1 text-xs text-muted-foreground"
                    style={{
                      gridTemplateColumns: `repeat(${calendar.weeks.length}, 0.75rem)`,
                    }}
                    aria-hidden
                  >
                    {calendar.monthLabels.map((month) => (
                      <span
                        key={`${month.weekIndex}:${month.label}`}
                        className="truncate"
                        style={{
                          gridColumn: `${month.weekIndex + 1} / span ${month.weekSpan}`,
                        }}
                      >
                        {month.label}
                      </span>
                    ))}
                  </div>
                  <div className="flex gap-2">
                    <div
                      className="grid w-7 grid-rows-7 gap-1 text-[10px] leading-3 text-muted-foreground"
                      aria-hidden
                    >
                      <span />
                      <span>{t("Mon")}</span>
                      <span />
                      <span>{t("Wed")}</span>
                      <span />
                      <span>{t("Fri")}</span>
                      <span />
                    </div>
                    <div
                      className="grid gap-1"
                      role="grid"
                      aria-label={t("Request activity calendar")}
                    >
                      {Array.from({ length: 7 }, (_, dayIndex) => (
                        <div key={dayIndex} className="flex gap-1" role="row">
                          {calendar.weeks.map((week, weekIndex) => {
                            const day = week.days[dayIndex];
                            if (!day) {
                              return (
                                <span
                                  key={`empty:${weekIndex}`}
                                  className="size-3"
                                  aria-hidden
                                />
                              );
                            }
                            const formattedDate = formatUtcDate(day.date, locale);
                            const label =
                              day.request_count === 1
                                ? t("1 request on {date}", {
                                    date: formattedDate,
                                  })
                                : t("{count} requests on {date}", {
                                    count: day.request_count.toLocaleString(locale),
                                    date: formattedDate,
                                  });
                            const level = intensityLevel(
                              day.request_count,
                              calendar.maximum,
                            );
                            return (
                              <Tooltip key={day.date}>
                                <TooltipTrigger
                                  render={
                                    <span
                                      role="gridcell"
                                      tabIndex={day.request_count > 0 ? 0 : -1}
                                      aria-label={label}
                                      className={cn(
                                        "size-3 rounded-[2px] ring-1 ring-inset ring-border/40 outline-none focus-visible:ring-2 focus-visible:ring-ring",
                                        INTENSITY_CLASSES[level],
                                      )}
                                    />
                                  }
                                />
                                <TooltipContent>{label}</TooltipContent>
                              </Tooltip>
                            );
                          })}
                        </div>
                      ))}
                    </div>
                  </div>
                </div>
              </div>

              <div className="flex items-center justify-end gap-2 text-xs text-muted-foreground">
                <span>{t("Less")}</span>
                {INTENSITY_CLASSES.map((className, index) => (
                  <span
                    key={className}
                    className={cn(
                      "size-3 rounded-[2px] ring-1 ring-inset ring-border/40",
                      className,
                    )}
                    aria-label={t("Activity level {level}", { level: index })}
                  />
                ))}
                <span>{t("More")}</span>
              </div>
            </CardContent>
          </Card>
        </div>
      ) : null}
    </AsyncResource>
  );
}
