import { useState } from "react";
import { Check, Copy, Eye, EyeOff } from "lucide-react";
import { toast } from "sonner";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@/components/ui/input-group";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { maskApiKey } from "@/lib/api-keys";
import { cn } from "@/lib/utils";
import { useI18n } from "@/app/i18n";

interface ApiKeyValueProps {
  value: string;
  className?: string;
}

/** A retrievable API key that stays masked until the user explicitly reveals it. */
export function ApiKeyValue({ value, className }: ApiKeyValueProps) {
  const [revealed, setRevealed] = useState(false);
  const [copied, setCopied] = useState(false);
  const { t } = useI18n();

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      toast.success(t("API key copied"));
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      toast.error(t("Copy failed"));
    }
  };

  return (
    <InputGroup
      className={cn("min-w-72", className)}
      onClick={(event) => event.stopPropagation()}
    >
      <InputGroupInput
        readOnly
        value={revealed ? value : maskApiKey(value)}
        aria-label={t("API key value")}
        autoComplete="off"
        className="font-mono text-xs"
      />
      <InputGroupAddon align="inline-end">
        <Tooltip>
          <TooltipTrigger
            render={
              <InputGroupButton
                size="icon-xs"
                aria-label={t(revealed ? "Hide full API key" : "Show full API key")}
                onClick={() => setRevealed((current) => !current)}
              />
            }
          >
            {revealed ? <EyeOff /> : <Eye />}
          </TooltipTrigger>
          <TooltipContent>
            {t(revealed ? "Hide full API key" : "Show full API key")}
          </TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger
            render={
              <InputGroupButton
                size="icon-xs"
                aria-label={t("Copy API key")}
                onClick={copy}
              />
            }
          >
            {copied ? <Check /> : <Copy />}
          </TooltipTrigger>
          <TooltipContent>{t(copied ? "Copied" : "Copy API key")}</TooltipContent>
        </Tooltip>
      </InputGroupAddon>
    </InputGroup>
  );
}
