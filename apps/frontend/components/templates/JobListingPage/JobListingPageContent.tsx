import NavigationLink from "@/components/global/Navigation/NavigationLink";
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
      </div>

      {/* Job details */}
      <div className="relative flex w-full">
        <div className="relative flex-1 lg:pr-[68px]">
          <RichText value={props?.post} usage="textPage" />
        </div>
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
