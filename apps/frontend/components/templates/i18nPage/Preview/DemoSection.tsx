"use client";
import Preview from "@/components/templates/i18nPage/Preview";
import Code from "@/components/templates/i18nPage/Preview/CodeSwitcher";
import { Play } from "@/components/templates/i18nPage/Preview/Play";
import { Steps } from "@/components/templates/i18nPage/Preview/types";
import { AnimatePresence, motion, useInView } from "framer-motion";
import { useCallback, useEffect, useRef, useState } from "react";
import { Timeline } from "./Timeline";

export default function DemoSection() {
  const [step, setStep] = useState<Steps>(Steps.Analyzing);
  const [isAnimating, setIsAnimating] = useState(false);
  const [resetKey, setResetKey] = useState(0);
  const [hasPlayed, setHasPlayed] = useState(false);

  const ref = useRef(null);
  const isInView = useInView(ref, { margin: "-50px 0px" });

  const handleAnimationReset = useCallback(() => {
    setResetKey((prevKey) => prevKey + 1);
    setStep(Steps.Analyzing);
    setIsAnimating(true);
  }, []);

  const handleStepComplete = () => {
    setStep(Steps.Finish);
    setTimeout(() => {
      setIsAnimating(false);
    }, 5000);
  };

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === " ") {
        event.preventDefault();

        if (!isAnimating) {
          handleAnimationReset?.();
        }
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [handleAnimationReset, isAnimating]);

  useEffect(() => {
    if (isInView && !isAnimating && !hasPlayed) {
      const timeout = setTimeout(() => {
        handleAnimationReset();
        setHasPlayed(true);
      }, 1500);

      return () => clearTimeout(timeout);
    }
  }, [isInView, isAnimating, hasPlayed, handleAnimationReset]);

  return (
    <div className="relative z-[60] w-full max-w-full" ref={ref}>
      <main
        key={`Timeline-${resetKey}`} // Force re-mount of the main component
        className="relative mx-auto flex max-w-[1152px] flex-col gap-1 overflow-hidden rounded-xl border border-border-light bg-zinc-200 p-1 shadow-2xl dark:border-border-dark dark:bg-gray-950"
      >
        <AnimatePresence>
          {!isAnimating && (
            <Play
              className="absolute inset-0 z-50 flex items-center justify-center bg-white/30 transition-colors hover:bg-white/10 dark:bg-gray-950/20 dark:hover:bg-gray-950/10"
              onClick={handleAnimationReset}
            />
          )}
        </AnimatePresence>

        <Timeline
          key={`Timeline-${resetKey}`}
          step={step}
          isAnimating={isAnimating}
        />
        <motion.div className="flex flex-col gap-1 md:h-[500px] md:flex-row">
          <div className="relative order-2 flex flex-col overflow-hidden rounded-l md:order-1 md:w-[40%]">
            <Code
              key={`Code-${resetKey}`}
              step={step}
              isAnimating={isAnimating}
              setStep={setStep}
            />
          </div>

          <div className="pointer-events-none relative order-1 flex-1 select-none overflow-hidden rounded-r">
            <Preview
              key={`Preview-${resetKey}`}
              step={step}
              isAnimating={isAnimating}
              onStepComplete={handleStepComplete}
            />
          </div>
        </motion.div>
      </main>
    </div>
  );
}
