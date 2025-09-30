import GradientBlob from "@/components/shared/GradientBlob";
import GradientBorderBox from "@/components/shared/GradientBorderBox";
import InfiniteSlider from "@/components/shared/InfiniteSlider";
import TrackedLinkButton from "@/components/shared/TrackedLinkButton";
import BookOpenLinkButton from "@/components/shared/animated-icons/BookOpenLinkButton";
import CobeSection from "./Cobe";
import type { I18NPageProps } from "./Page";
import DemoSection from "./Preview/DemoSection";
import ShiningLines from "./Preview/ShinningLines";
import FeaturesSection from "./Sections/features";
import CaseStudySection from "./Sections/caseStudy";
import BentoGridSection from "./Sections/bentoGrid";

export default function I18NPageSections({ data }: I18NPageProps) {
  return (
    <>
      <noscript>
        <iframe
          src="https://www.googletagmanager.com/ns.html?id=GTM-PBSBXXDS"
          height="0"
          width="0"
          style={{ display: "none", visibility: "hidden" }}
        />
      </noscript>
      {/* Hero */}
      <div className="relative w-full overflow-x-clip px-xl py-[80px] pt-[calc(var(--header-height)+5rem)] lg:w-auto lg:overflow-x-visible lg:px-[80px]">
        <div className="pointer-events-none absolute left-0 top-0 z-[1] h-full w-full">
          <div className="pointer-events-none relative h-full w-full">
            <GradientBlob style="planet" />
            <GradientBlob style="ellipse" />
          </div>
        </div>

        <ShiningLines numberOfLines={6} />

        <div className="relative z-20 flex flex-col items-center justify-center text-center">
          <div className="flex flex-col items-center justify-center">
            <h1 className="xl-heading max-w-96 font-bold lg:max-w-[800px]">
              {data?.hero?.title}
            </h1>
            {data?.hero?.subtitle && (
              <p className="body-l mb-10 mt-4 max-w-lg lg:mt-6 lg:max-w-2xl">
                {data?.hero?.subtitle}
              </p>
            )}
          </div>
          <div className="flex justify-between gap-4">
            {data.hero?.ctas?.[0] && (
              <TrackedLinkButton
                key={data.hero?.ctas?.[0]._key}
                href={data.hero?.ctas?.[0].link}
              >
                {data.hero?.ctas?.[0].label}
              </TrackedLinkButton>
            )}
            {data.hero?.ctas?.[1] && (
              <>
                <BookOpenLinkButton
                  intent="secondary"
                  href={data.hero?.ctas?.[1].link}
                >
                  {data.hero?.ctas?.[1].label}
                </BookOpenLinkButton>
              </>
            )}
          </div>
        </div>
      </div>

      <DemoSection />

      {data?.hero?.logoCarousel?.logos?.length && (
        <div className="relative z-0">
          <GradientBorderBox
            extend={{
              corners: { tr: true, br: true, bl: true, tl: true },
              orientation: "vertical",
              extraExtension: ["top-right", "top-left"],
            }}
            className="mx-auto"
          >
            <div className="relative mt-28 max-w-6xl py-10">
              <div className=" flex flex-col items-center gap-10 lg:flex-row lg:gap-12 lg:pl-28">
                <h2 className="body-s-medium w-full max-w-[222px] text-center !font-medium lg:min-w-[220px] lg:max-w-[221px] lg:text-left">
                  {data?.hero?.logoCarousel.title}
                </h2>
                {data?.hero?.logoCarousel?.logos && (
                  <InfiniteSlider
                    direction="left"
                    items={data?.hero?.logoCarousel?.logos}
                  />
                )}
              </div>
            </div>
          </GradientBorderBox>
        </div>
      )}
      <BentoGridSection />
      <FeaturesSection />
      <CaseStudySection />
      <CobeSection />
    </>
  );
}
