"use client";
import React, { useRef } from "react";
import { motion, useInView } from "framer-motion";
import { SeparatorY, SeparatorX } from "./separators";
import { cn } from "@/utils";
import Tag from "@/components/shared/Tag";

type Feature = {
  id: string;
  label: string;
  title: string;
  description: string;
  lightImage?: string;
  darkImage?: string;
  alt: string;
};

type Props = {
  features?: Feature[];
};

const defaultFeatures: Feature[] = [
  {
    id: "plan",
    label: "Plan",
    title: "Uncover issues early",
    lightImage: "/static/i18n/plan-light.svg",
    darkImage: "/static/i18n/plan-dark.svg",
    description:
      "Get a clear table view of team issues to plan projects with confidence from day one.",
    alt: "Plan visualization showing hard-coded strings metrics",
  },
  {
    id: "track",
    label: "Track",
    title: "Monitor progress by team",
    lightImage: "/static/i18n/track-light.svg",
    darkImage: "/static/i18n/track-dark.svg",
    description:
      "See how teams are doing over time, spot risks, and allocate resources early.",
    alt: "Track visualization showing team progress burndown chart",
  },
  {
    id: "align",
    label: "Align",
    title: "Speak business language",
    lightImage: "/static/i18n/align-light.svg",
    darkImage: "/static/i18n/align-dark.svg",
    description:
      "Use custom formulas to turn i18n issues from the codebase into business metrics, building trust and aligning with XFN teams and leadership.",
    alt: "Align visualization showing cost savings with codemod",
  },
];

// Animation variants for fade in from bottom
const fadeInFromBottom = {
  initial: {
    opacity: 0,
    y: 50,
  },
  animate: {
    opacity: 1,
    y: 0,
    transition: {
      duration: 0.6,
      ease: "easeOut",
    },
  },
};

// Animation variants for left column (text content) - animates first
const leftColumnAnimation = {
  initial: {
    opacity: 0,
    y: 50,
  },
  animate: {
    opacity: 1,
    y: 0,
    transition: {
      duration: 0.6,
      ease: "easeOut",
      delay: 0,
    },
  },
};

// Animation variants for right column (image) - animates with delay
const rightColumnAnimation = {
  initial: {
    opacity: 0,
    y: 50,
  },
  animate: {
    opacity: 1,
    y: 0,
    transition: {
      duration: 0.6,
      ease: "easeOut",
      delay: 0.4,
    },
  },
};

// Individual feature component with animation
const FeatureItem = ({
  feature,
  index,
  features,
}: {
  feature: Feature;
  index: number;
  features: Feature[];
}) => {
  const ref = useRef(null);
  const isInView = useInView(ref, { once: true, amount: 0.5 });

  return (
    <>
      <article
        ref={ref}
        className="relative grid grid-cols-1 gap-8 md:grid-cols-2 md:gap-12"
      >
        <motion.div
          variants={leftColumnAnimation}
          initial="initial"
          animate={isInView ? "animate" : "initial"}
          className="flex flex-col justify-center space-y-4 p-4 md:px-12"
        >
          <div className="flex items-center">
            <Tag intent="primary">{feature.label}</Tag>
          </div>
          <div className="space-y-3">
            <h3 className="l-heading">{feature.title}</h3>
            <p className="body-l max-w-[400px]">{feature.description}</p>
          </div>
        </motion.div>

        <motion.figure
          variants={rightColumnAnimation}
          initial="initial"
          animate={isInView ? "animate" : "initial"}
          className="relative flex w-full max-w-full items-center justify-center overflow-hidden md:py-12 md:pl-0 md:pr-6"
        >
          <>
            {/* Light Mode Image */}
            {feature.lightImage && (
              <img
                src={feature.lightImage}
                alt={feature.alt}
                className="block aspect-[450/390] h-full w-full rounded-2xl object-cover dark:hidden"
              />
            )}
            {/* Dark Mode Image */}
            {feature.darkImage && (
              <img
                src={feature.darkImage}
                alt={feature.alt}
                className="hidden aspect-[450/390] h-full w-full rounded-2xl object-cover shadow-xl dark:block"
              />
            )}
            {/* Fallback */}
            {feature.lightImage && !feature.darkImage && (
              <img
                src={feature.lightImage}
                alt={feature.alt}
                className="aspect-[450/390] h-full w-full rounded-2xl object-cover dark:hidden"
              />
            )}
          </>
        </motion.figure>
        <SeparatorX
          className="bottom-4 left-1/2 top-4 hidden h-auto md:block"
          aria-hidden="true"
        />
      </article>

      {index !== features.length - 1 && <SeparatorY variant="half" simple />}
    </>
  );
};

const FeaturesSection = ({ features = defaultFeatures }: Props) => {
  return (
    <section
      className="relative w-full overflow-x-hidden px-6 py-[80px] pt-[96px] md:pt-[140px] lg:px-[80px]"
      aria-labelledby="features-heading"
      role="region"
    >
      <header className="mx-auto mb-16 flex max-w-6xl flex-col items-center gap-4 text-center">
        <h2 className="l-heading font-bold" id="features-heading">
          See through your codebase i18n health
        </h2>
        <p className="body-l max-w-2xl">
          Stop firefighting i18n issues right before launch. Get real-time,
          business aligned insights from day one.
        </p>
      </header>

      <SeparatorY edge="top" variant="half" />

      <div
        className={cn(
          "mx-auto gap-12",
          "relative mx-auto w-full max-w-6xl",
          "border-x border-border-light dark:border-border-dark",
        )}
        role="group"
        aria-label="Key features"
      >
        {features.map((feature, index) => (
          <FeatureItem
            key={feature.id}
            feature={feature}
            index={index}
            features={features}
          />
        ))}
      </div>
      <SeparatorY edge="bottom" variant="half" />
    </section>
  );
};

export default FeaturesSection;
