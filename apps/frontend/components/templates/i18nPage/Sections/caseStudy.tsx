"use client";
import Image from "next/image";
import Link from "next/link";
import { motion, useInView, LayoutGroup } from "motion/react";
import { ArrowUpRight } from "lucide-react";
import TextRotate from "../Cobe/TextRotate";
import { useRef } from "react";

const caseStudy = {
  quote:
    "“Codemod gave us the visibility we needed when piloting our i18n effort. We could focus on the business while their team handled the heavy lifting. Their platform automated large-scale code updates across multiple teams. We’re especially excited about the new AI orchestration workflow!”",
  authorName: "Charles-Antoine de Salaberry",
  authorPosition: "Tech lead at Padoa",
  authorImage: {
    src: "/static/i18n/casestudy-author.jpg",
  },
  image: {
    src: "/static/i18n/casestudy-logo.jpg",
  },
};

const kpi = {
  value: 10,
  label: "Faster time to market",
  description: "Unlock new markets and growth opportunities.",
};

const cta = {
  label: "Read case study",
  href: "https://codemod.com/blog/padoa",
};

export default function CaseStudySection() {
  const caseStudySectionRef = useRef<HTMLDivElement>(null);
  const inView = useInView(caseStudySectionRef, {
    once: true,
    margin: "0% 0%",
  });

  return (
    <LayoutGroup>
      <section ref={caseStudySectionRef} className="px-xl lg:px-[80px]">
        <div className="mx-auto max-w-7xl rounded-3xl bg-accent p-2.5">
          <div className="grid grid-cols-1 gap-8 lg:grid-cols-3 lg:items-stretch">
            {/* Left: Testimonial Card */}
            <div className="col-span-2 flex flex-col items-start gap-8 rounded-2xl bg-white p-6 shadow-sm lg:p-12 lg:pb-8 lg:pr-8 xl:p-16 xl:pb-12 xl:pr-14 dark:bg-background-dark">
              <svg
                xmlns="http://www.w3.org/2000/svg"
                className="block h-8 text-[#0046da] dark:text-[#3E7BFF] w-auto"
                viewBox="0 0 88 32"
                fill="none"
              >
                <path
                  fill="currentColor"
                  d="M7.846 7.526a7.4 7.4 0 0 0-4.034 1.206A1.98 1.98 0 0 0 1.98 7.526C.887 7.526 0 8.398 0 9.474v20.578C0 31.127.887 32 1.98 32s1.98-.873 1.98-1.948v-6.73a7.37 7.37 0 0 0 3.884 1.113c4.325 0 7.843-3.793 7.843-8.454S12.17 7.525 7.844 7.525zm0 13.013c-2.14 0-3.884-2.045-3.884-4.559 0-2.513 1.743-4.558 3.884-4.558s3.883 2.045 3.883 4.558-1.743 4.56-3.883 4.56M25.925 7.565c-4.325 0-7.844 3.793-7.844 8.455 0 4.661 3.519 8.454 7.844 8.454a7.4 7.4 0 0 0 4.034-1.206 1.98 1.98 0 0 0 1.831 1.206c1.093 0 1.98-.872 1.98-1.948V16.02c0-4.662-3.518-8.455-7.843-8.455zm0 13.013c-2.14 0-3.884-2.045-3.884-4.558s1.743-4.56 3.884-4.56 3.883 2.046 3.883 4.56-1.743 4.558-3.883 4.558M49.863 0c-1.093 0-1.98.873-1.98 1.948v6.73A7.37 7.37 0 0 0 44 7.566c-4.325 0-7.844 3.793-7.844 8.455 0 4.661 3.52 8.454 7.844 8.454a7.4 7.4 0 0 0 4.034-1.206 1.98 1.98 0 0 0 1.831 1.206c1.093 0 1.98-.872 1.98-1.948V1.948C51.846.873 50.96 0 49.866 0zM44 20.578c-2.14 0-3.883-2.045-3.883-4.558s1.742-4.56 3.883-4.56 3.883 2.046 3.883 4.56-1.742 4.558-3.883 4.558M62.076 7.565c-4.325 0-7.844 3.793-7.844 8.455 0 4.661 3.52 8.454 7.844 8.454 4.325 0 7.844-3.793 7.844-8.454S66.4 7.565 62.076 7.565m0 13.013c-2.14 0-3.883-2.045-3.883-4.558s1.742-4.56 3.883-4.56 3.883 2.046 3.883 4.56-1.742 4.558-3.883 4.558M87.998 16.02c0-4.662-3.519-8.455-7.844-8.455s-7.843 3.793-7.843 8.455c0 4.661 3.518 8.454 7.843 8.454a7.4 7.4 0 0 0 4.034-1.206 1.98 1.98 0 0 0 1.832 1.206c1.093 0 1.98-.872 1.98-1.948V16.02zm-7.844 4.558c-2.14 0-3.883-2.045-3.883-4.558s1.743-4.56 3.883-4.56 3.883 2.046 3.883 4.56-1.742 4.558-3.883 4.558"
                />
              </svg>

              <motion.p
                className="text-[20px] lg:text-[24px] leading-[1.25] !font-bold"
                layout
              >
                <TextRotate
                  texts={["", caseStudy.quote]}
                  auto={inView}
                  staggerFrom={"first"}
                  staggerDuration={0.03}
                  initial={{ opacity: 0, x: 10 }}
                  animate={{ opacity: 1, x: 0 }}
                  exit={{ opacity: 0, x: -10 }}
                  transition={{ type: "spring", damping: 30, stiffness: 400 }}
                  elementLevelClassName="inline whitespace-nowrap"
                  loop={false}
                  rotationInterval={50}
                  splitBy="words"
                />
              </motion.p>

              <div className="flex items-center gap-3">
                <motion.div
                  className="relative w-8 h-8 rounded-full dark:bg-tertiary-light bg-tertiary-dark"
                  initial={{ opacity: 0 }}
                  animate={inView && { opacity: 1 }}
                  transition={{ delay: 1.5 }}
                >
                  {caseStudy.authorImage.src && (
                    <Image
                      className="rounded-full w-8 h-8 object-cover"
                      width={100}
                      height={100}
                      src={caseStudy.authorImage.src}
                      alt={caseStudy.authorName}
                    />
                  )}
                  {caseStudy.image.src && (
                    <Image
                      width={64}
                      height={64}
                      src={caseStudy.image.src}
                      className="h-4 w-4 absolute bottom-0 right-0 z-10 rounded-full dark:bg-white/20 bg-black/20 object-cover"
                      alt={caseStudy.authorName}
                    />
                  )}
                </motion.div>

                <div className="flex items-center flex-row flex-wrap gap-1">
                  {caseStudy.authorName && (
                    <cite className="body-s-medium font-medium not-italic">
                      <TextRotate
                        texts={["", caseStudy.authorName]}
                        auto={inView}
                        staggerFrom={"first"}
                        staggerDuration={0.04}
                        initial={{ opacity: 0, x: 10 }}
                        animate={{ opacity: 1, x: 0 }}
                        exit={{ opacity: 0, x: -10 }}
                        transition={{
                          type: "spring",
                          damping: 30,
                          stiffness: 400,
                        }}
                        loop={false}
                        rotationInterval={1000}
                        splitBy="words"
                      />
                    </cite>
                  )}

                  {caseStudy.authorPosition && (
                    <span className="body-s-medium font-medium text-secondary-light dark:text-secondary-dark">
                      <TextRotate
                        texts={["", caseStudy.authorPosition]}
                        auto={inView}
                        staggerFrom={"first"}
                        staggerDuration={0.04}
                        initial={{ opacity: 0, x: 10 }}
                        animate={{ opacity: 1, x: 0 }}
                        exit={{ opacity: 0, x: -10 }}
                        transition={{
                          type: "spring",
                          damping: 30,
                          stiffness: 400,
                        }}
                        loop={false}
                        rotationInterval={1500}
                        splitBy="words"
                      />
                    </span>
                  )}
                </div>
              </div>
            </div>

            {/* Right: KPI + CTA */}
            <div className="flex flex-col justify-end p-2 pb-12 text-black">
              <div>
                <h3 className="xl-heading !font-bold">{kpi.value}x</h3>
                <p className="m-heading leading-4 !font-bold">{kpi.label}</p>
                <p className="body-m !font-medium max-w-sm mt-4">
                  {kpi.description}
                </p>
              </div>
              <div className="mt-8">
                <Link
                  href={cta.href}
                  target="_blank"
                  className="body-m group inline-flex items-center gap-2 !font-bold hover:opacity-90"
                >
                  {cta.label}
                  <ArrowUpRight className="h-4 w-4 transition-transform ease-out group-hover:-translate-y-[2px] group-hover:translate-x-[2px]" />{" "}
                </Link>
              </div>
            </div>
          </div>
        </div>
      </section>
    </LayoutGroup>
  );
}
