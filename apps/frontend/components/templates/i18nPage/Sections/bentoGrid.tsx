"use client";
import React from "react";
import { motion, useInView } from "framer-motion";
import { useRef } from "react";
import { cn } from "@/utils";

type BentoCard = {
  id: string;
  title: string;
  description: string;
  lightImage?: string;
  darkImage?: string;
  alt: string;
  size: "large" | "medium";
};

type Props = {
  title?: string;
  subtitle?: string;
  cards?: BentoCard[];
};

const defaultCards: BentoCard[] = [
  {
    id: "customize",
    title: "Customize automation recipes",
    description:
      "Use expert-built multi-step workflows that adapt to your codebase and organizational needs.",
    alt: "Customize automation recipes visualization",
    lightImage: "/static/i18n/customize-light.png",
    darkImage: "/static/i18n/customize-dark.png",
    size: "large",
  },
  {
    id: "orchestrate",
    title: "Orchestrate Pull Requests centrally",
    description:
      "Generate safe, easy-to-review PRs for each team, keep them up to date, send reminders, and track progress in one place.",
    alt: "Orchestrate Pull Requests visualization",
    lightImage: "/static/i18n/orchestrate-light.svg",
    darkImage: "/static/i18n/orchestrate-dark.svg",
    size: "medium",
  },
  {
    id: "transform",
    title: "Transform code reliably with AI smarts",
    description:
      "Codemod agents use compilers for reliable changes and AI for human-like tweaks, handling complex i18n code transformations like interpolations, variables, and patterns.",
    alt: "Transform code with AI visualization",
    lightImage: "/static/i18n/transform-light.svg",
    darkImage: "/static/i18n/transform-dark.svg",
    size: "medium",
  },
];

// Animation variants
const fadeInUp = {
  initial: {
    opacity: 0,
    y: 30,
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

const staggerContainer = {
  animate: {
    transition: {
      staggerChildren: 0.1,
    },
  },
};

const BentoCard = ({ card }: { card: BentoCard }) => {
  const ref = useRef(null);
  const isInView = useInView(ref, { once: true, amount: 0.8 });

  return (
    <motion.article
      ref={ref}
      variants={fadeInUp}
      initial="initial"
      animate={isInView ? "animate" : "initial"}
      className={cn(
        "group relative overflow-hidden rounded-2xl border border-border-light bg-white p-6 shadow-sm dark:border-border-dark dark:bg-[#050D15] dark:shadow-xl",
        card.size === "large" && "md:col-span-1 md:row-span-2",
        card.size === "medium" && "md:col-span-1 md:row-span-1"
      )}
    >
      <div
        className={cn(
          "flex h-full flex-col",
          card.id === "orchestrate" ? "flex-col-reverse" : ""
        )}
      >
        {/* Text Content */}
        <div
          className={cn(
            "relative z-10 flex flex-1 flex-col gap-2",
            card.id === "customize" &&
              "mb-8 flex-auto items-center text-center md:pt-12",
            card.id === "transform" && "mb-4"
          )}
        >
          <h3 className="font-bold text-2xl text-primary-light dark:text-primary-dark">
            {card.title}
          </h3>
          <p
            className={cn(
              "text-sm text-secondary-light dark:text-secondary-dark",
              card.id === "customize" && "mx-auto max-w-[330px]"
            )}
          >
            {card.description}
          </p>
        </div>

        {/* Image/Visual Placeholder */}
        <div
          className={cn(
            "relative w-full [mask-image:linear-gradient(to_bottom,black_80%,transparent_100%)] [mask-repeat:no-repeat] [mask-size:100%_100%]",
            card.id === "customize" &&
              "aspect-[861/1004] flex-1 [mask-image:linear-gradient(to_bottom,black_80%,transparent_100%)]",

            card.id === "orchestrate" && "aspect-[1085/330]",
            card.id === "transform" && "-mb-4 aspect-[1085/660]"
          )}
        >
          {/* Light/Dark Image Support */}
          {card.lightImage && (
            <img
              src={card.lightImage}
              alt={card.alt}
              className={cn(
                "absolute inset-x-0 bottom-0 top-0 rounded-xl border border-border-light dark:border-border-dark",
                "block h-full w-full object-cover dark:hidden",
                card.id === "orchestrate" && "object-top",
                card.id === "transform" && "object-bottom"
              )}
            />
          )}
          {card.darkImage && (
            <img
              src={card.darkImage}
              alt={card.alt}
              className={cn(
                "absolute inset-x-0 bottom-0 top-0 rounded-xl border border-border-light dark:border-border-dark",
                "hidden h-full w-full object-cover dark:block",
                card.id === "orchestrate" && "object-top",
                card.id === "transform" && "object-bottom"
              )}
            />
          )}
        </div>
      </div>
    </motion.article>
  );
};

const BentoGridSection = ({
  title = "Automate tedious coding at any scale",
  subtitle = "Stop wasting time on string replacements. Let Codemod agents handle i18n code changes while coordinating the work across teams.",
  cards = defaultCards,
}: Props) => {
  const containerRef = useRef(null);
  const isInView = useInView(containerRef, { once: true, amount: 0.1 });

  return (
    <section
      ref={containerRef}
      className="relative w-full overflow-x-hidden px-6 py-[80px] pt-[96px] md:pt-[140px] lg:px-[80px]"
      aria-labelledby="bento-heading"
      role="region"
    >
      <motion.div
        variants={staggerContainer}
        initial="initial"
        animate={isInView ? "animate" : "initial"}
        className="mx-auto max-w-7xl"
      >
        {/* Header */}
        <motion.header
          variants={fadeInUp}
          className="mx-auto mb-16 flex max-w-6xl flex-col items-center gap-4 text-center"
        >
          <h2 id="bento-heading" className="l-heading font-bold">
            {title}
          </h2>
          <p className="body-l mx-auto max-w-2xl">{subtitle}</p>
        </motion.header>

        {/* Bento Grid */}
        <div className="grid grid-cols-1 gap-4 md:grid-cols-[45%_55%]">
          {cards.map((card) => (
            <BentoCard key={card.id} card={card} />
          ))}
        </div>
      </motion.div>
    </section>
  );
};

export default BentoGridSection;
