import { cn } from "@/lib/utils"
import { translate } from "@/app/i18n"
import { Loader2Icon } from "lucide-react"
import { motion } from "motion/react"
import { usePrefersReducedMotion } from "@/hooks/use-prefers-reduced-motion"

const MotionLoader = motion.create(Loader2Icon)

function Spinner({ className, ...props }: React.ComponentProps<typeof MotionLoader>) {
  const reduceMotion = usePrefersReducedMotion()
  return (
    <MotionLoader
      data-slot="spinner"
      role="status"
      aria-label={translate("Loading")}
      className={cn("size-4", className)}
      animate={{ rotate: reduceMotion ? 0 : 360 }}
      transition={reduceMotion ? { duration: 0 } : { duration: 1, repeat: Infinity, ease: "linear" }}
      {...props}
    />
  )
}

export { Spinner }
