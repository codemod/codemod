import NavigationLink from "@/components/global/Navigation/NavigationLink";
import CtaCard from "@/components/shared/CtaCard";
import Icon from "@/components/shared/Icon";
import RelatedLinks from "@/components/shared/RelatedLinks";
import { RichText } from "@/components/shared/RichText";
import type { Job } from "@/types";

export default function JobListingPageContent(props: Job) {
  return (
    <div className="relative flex w-full flex-col items-start justify-center gap-l px-s pb-xl pt-[calc(var(--header-height)+24px)] lg:gap-2xl lg:px-[128px] lg:pb-[80px]">
      {/* Link back to /careers */}
      <div className="w-full">
        <NavigationLink
          href="/careers"
          className="body-s-medium flex items-center gap-xs font-medium text-secondary-light dark:text-secondary-dark"
        >
          <Icon name="arrow-left" />
          <span>{props.globalLabels?.backToIndex || "Back to Careers"}</span>
        </NavigationLink>
      </div>

      {/* Header */}
      <div className="flex w-full flex-col items-start gap-l lg:gap-s">
        <div className="flex items-center gap-m">
          <span className="body-s-medium block rounded-[4px] border-[1px] border-border-light px-xs py-xxs font-medium dark:border-border-dark">
            {props?.department}
          </span>
          <span className="body-s-medium block font-medium text-secondary-light dark:text-secondary-dark">
            {props?.location}
          </span>
        </div>
        <h1 className="xl-heading">{props?.title}</h1>
        <div className="lg:hidden">
          <CtaCard
            title={props.globalLabels?.applyToPosition || "Apply to position"}
            description={
              props.globalLabels?.applyToPositionDescription ||
              "Ready to feel the rush?"
            }
            ctaText={props.globalLabels?.applyToPositionCTA || "Apply"}
            href="#apply-to-job"
          />
        </div>
      </div>

      {/* Job details */}
      <div className="relative flex w-full">
        <div className="relative flex-1 lg:pr-[68px]">
          <RichText value={props?.post} usage="textPage" />
          <div className="absolute right-0 top-0 hidden h-full w-[1px] bg-gradient-to-b from-transparent via-[#0b151e39] via-10% to-transparent to-95%  lg:flex dark:via-[#ffffff33]" />
        </div>

        <aside className="sticky top-8 hidden h-fit w-1/3 px-s pl-[52px] lg:flex">
          <div className="flex w-full flex-col">
            <div className="mb-l">
              <CtaCard
                title={
                  props.globalLabels?.applyToPosition || "Apply to position"
                }
                description={
                  props.globalLabels?.applyToPositionDescription ||
                  "Ready to feel the rush?"
                }
                ctaText={props.globalLabels?.applyToPositionCTA || "Apply"}
                href="#apply-to-job"
              />
            </div>
            <div className="h-[1px] w-full bg-gradient-to-r from-transparent via-[#0b151e39] to-transparent dark:via-emphasis-dark" />
            {props?.relatedPositions ? (
              <RelatedLinks
                className="mt-l"
                title={props.globalLabels?.relatedJobs || "Related positions"}
                links={props?.relatedPositions.map((position) => ({
                  title: position?.title,
                  href: position?.pathname?.current.split("/")[1],
                }))}
              />
            ) : null}
          </div>
        </aside>
      </div>


      {props?.relatedPositions ? (
        <RelatedLinks
          className="py-2xl lg:hidden"
          title={props.globalLabels?.relatedJobs || "Related positions"}
          links={props?.relatedPositions.map((position) => ({
            title: position?.title,
            href: position?.pathname?.current.split("/")[1],
          }))}
        />
      ) : null}
    </div>
  );
}
