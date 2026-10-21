import { cn } from "@/lib/utils"
import { motion } from "motion/react"
import { usePrefersReducedMotion } from "@/hooks/use-prefers-reduced-motion"

function Skeleton({ className, ...props }: React.ComponentProps<typeof motion.div>) {
  const reduceMotion = usePrefersReducedMotion()
  return (
    <motion.div
      data-slot="skeleton"
      className={cn("rounded-md bg-muted", className)}
      animate={{ opacity: reduceMotion ? 1 : [1, 0.5, 1] }}
      transition={reduceMotion ? { duration: 0 } : { duration: 2, repeat: Infinity, ease: "easeInOut" }}
      {...props}
    />
  )
}

export { Skeleton }
