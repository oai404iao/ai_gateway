import type { ReactNode } from "react";
import { Boxes, Calculator, Route } from "lucide-react";
import { useNavigate } from "react-router";
import { useI18n } from "@/app/i18n";
import { Button } from "@/components/ui/button";

export type ConfigurationLens = "routes" | "supply" | "models";

const destinations: Array<{
  lens: ConfigurationLens;
  path: string;
  label: string;
  icon: typeof Route;
}> = [
  {
    lens: "routes",
    path: "/admin/routing/model-rules",
    label: "Model rules",
    icon: Route,
  },
  {
    lens: "supply",
    path: "/admin/routing/channels",
    label: "Channels",
    icon: Boxes,
  },
  {
    lens: "models",
    path: "/admin/models",
    label: "Pricing models",
    icon: Calculator,
  },
];

export function ConfigurationNav({ lens }: { lens?: ConfigurationLens }) {
  const navigate = useNavigate();
  const { t } = useI18n();

  return (
    <nav
      className="flex flex-wrap gap-2"
      aria-label={t("Model routing configuration")}
    >
      {destinations.map((destination) => {
        const Icon = destination.icon;
        return (
          <Button
            key={destination.lens}
            type="button"
            size="sm"
            variant={lens === destination.lens ? "default" : "outline"}
            onClick={() => navigate(destination.path)}
          >
            <Icon data-icon="inline-start" />
            {t(destination.label)}
          </Button>
        );
      })}
    </nav>
  );
}

export function ConfigurationTableView({
  lens,
  children,
}: {
  lens: ConfigurationLens;
  children: ReactNode;
}) {
  return (
    <div className="space-y-4">
      <ConfigurationNav lens={lens} />
      {children}
    </div>
  );
}
