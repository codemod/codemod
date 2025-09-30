import { motion } from "framer-motion";
import CircularProgress from "../CircularProgress";

const childVariants = {
  initial: { opacity: 0, x: -20 },
  animate: {
    opacity: 1,
    x: 0,
    transition: {
      staggerChildren: 0.1,
      delay: 0.25,
    },
  },
  exit: { opacity: 0, x: 20 },
};

const stateMapping = [
  {
    label: "Analyzing",
    percentage: 40,
    description:
      "Found 1,034 hardcoded strings, Saved 4 weeks of engineering time.",
  },
  {
    label: "Transforming",
    percentage: 70,
    description: "I18n-ized 1,034 strings, Saved 8 weeks of engineering time.",
  },
  {
    label: "Translating",
    percentage: 90,
    description: "Work with one of our translation partners.",
  },
  {
    label: "Ready",
    percentage: 100,
    description:
      "Your project is fully internationalized and ready to translate.",
  },
];

export const Timeline = ({
  step,
  isAnimating,
}: {
  step: number;
  isAnimating: boolean;
}) => {
  const currentStep =
    !isAnimating && step === 3
      ? 3
      : Math.min(step > 1 ? step - 1 : 0, stateMapping.length - 1);
  const isAI = step < 3;

  return (
    <div className="sticky top-0 z-50 flex flex-wrap items-center gap-3 rounded-[6px] bg-gradient-to-r from-zinc-50 to-zinc-50/0 px-3 py-1 text-sm dark:from-white/10 dark:to-white/0">
      {currentStep < stateMapping.length && (
        <div className="flex items-center gap-3">
          <CircularProgress
            size={16}
            strokeWidth={2}
            percentage={stateMapping[currentStep]?.percentage || 0}
          />

          <motion.div
            key={`step-container-${currentStep}`}
            variants={childVariants}
            className="flex items-center gap-2"
          >
            <motion.span
              variants={childVariants}
              className="font-bold leading-7"
            >
              {stateMapping[currentStep]?.label}
            </motion.span>
            <motion.span
              variants={childVariants}
              className="text-xs text-zinc-600 dark:text-zinc-400"
            >
              {stateMapping[currentStep]?.description}
            </motion.span>

            {isAI && (
              <motion.span
                variants={childVariants}
                className="rounded bg-accent/50 p-0.5 text-xs text-black dark:bg-accent/10 dark:text-accent"
              >
                Automated by Codemod AI
              </motion.span>
            )}
          </motion.div>
        </div>
      )}
    </div>
  );
};
