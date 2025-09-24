import type { IconName } from "@/components/shared/Icon";
import type { SanityImageObject } from "@sanity/image-url/lib/types/types";
import type { BlocksBody, PublishStatus } from ".";
import type { CTAData, ImageWithAltFieldObject } from "./generic.types";

export type SanityImageWithAltField = SanityImageObject & {
  alt: string;
};

export type LogoDarkLight = {
  lightModeImage?: SanityImageWithAltField;
  darkModeImage?: SanityImageWithAltField;
};

export type PageHeroProps = {
  title: string;
  subtitle?: string;
  ctas?: (CTAData & { _key: string })[];
  logoCarousel?: {
    title: string;
    logos: (LogoDarkLight & {
      link?: string;
    })[];
  };
};
export type TextPageHeroProps = {
  title: string;
  lastUpdatedText?: string;
  tocTitle: string;
  body: BlocksBody;
};

export type PageCta = {
  title: string;
  paragraph: BlocksBody;
  cta: SanityStyledCta;
  _type: "pageCta";
};

export type PageCtaDouble = {
  title: string;
  leftSectionTitle: string;
  leftSectionParagraph: BlocksBody;
  leftSectionCta: SanityStyledCta;
  rightSectionTitle: string;
  rightSectionParagraph: BlocksBody;
  rightSectionIsNewsletter: boolean;
  rightSectionCta?: SanityStyledCta;
  privacyLink?: CTAData;
  _type: "pageCtaDouble";
};

export type PageCtaTriple = {
  title: string;
  paragraph: BlocksBody;
  splitPattern?: string;
  ctas: SanityStyledCta[];
  _type: "pageCtaTriple";
};

export type SanityStyledCta = {
  title: string;
  label: string;
  link: string;
  style: "primary" | "secondary";
  icon: IconName | "standard";
};
export type MediaTab = {
  _key: string;
  tabTitle?: string;
  mediaItem: (Omit<MuxVideoBlock, "caption"> | ImageWithAltFieldObject)[];
};

export type MuxVideo = {
  _type: "mux.video";
  asset: {
    playbackId?: string;
    resolution?: string;
  };
};

export type BlogTagInCard = {
  title?: string;
  slug?: string;
};

export type BlogArticleCardData = {
  _id: string;
  _type: string;
  title?: string;
  featuredImage?: ImageWithAltFieldObject;
  publishedAt?: string;
  tags?: BlogTagInCard[];
  preamble?: string;
  body: BlocksBody;
  pathname: string;
  slug: string;
  authors?: (BlogAuthor & { _key: string })[];
};

export type BlogAuthor = {
  name: string;
  details?: string;
  image?: SanityImageWithAltField;
};

/**
 * Portable Text Blocks
 */

export type MuxVideoBlock = {
  hasControls?: boolean;
  loop?: boolean;
  video?: MuxVideo;
  darkVideo?: MuxVideo;
  autoPlay?: boolean;
  caption?: string;
};
export type ImageBlock = {
  image: ImageWithAltFieldObject;
  caption?: string;
};

export type LinkedImageBlock = {
  image: ImageWithAltFieldObject;
  link?: { href?: string; label?: string };
  caption?: string;
};

export type TwitterEmbedBlock = {
  _type: "twitterEmbed";
  url?: string;
};

export type YoutubeVideoBlock = {
  _type: "youtubeVideo";
  caption?: string;
  youtubeUrl?: string;
};

export type CodeBlockProps = {
  language: string;
  highlightedLines?: number[];
  code: string;
  filename: string;
};

export type TableBlock = {
  _type: "ptTable";
  _key: string;
  table?: { rows: [{ cells?: string[]; _type: string; _key: string }] };
};

export type QuoteProps = {
  image?: SanityImageWithAltField;
  quote?: string;
  authorName?: string;
  authorPosition?: string;
  authorImage?: SanityImageWithAltField;
};

export type AutomationAPIListResponse = {
  name: string;
  slug: string;
  engine: string;
  author: string;
  tags: string[];
  verified: boolean;
  arguments: {
    kind: string;
    name: string;
    required: boolean;
  }[];

  updatedAt: string | null;
};

export type AutomationAPISearchResponse = {
  data?: AutomationResponse[];
  filters?: AutomationFilter[];
  total: number;
  page: number;
  size: number;
};

export type AutomationFilterIconValue = LogoDarkLight & {
  filterValue: string;
  icon?: string;
};

export type AutomationFilterType = {
  filterId: string;
  filterValues?: AutomationFilterIconValue[];
};

export type AutomationFilterIconDictionary = {
  filters?: AutomationFilterType[];
};

/**
 * @example [
 * string1: framework - ("@redwoodjs/core"),
 * string2: comparator - (">", "<", ">=", "<=", "=="),
 * string3: version - ("1.0.0")
 * ]
 */
export type AutomationApplicabilityValue = [string, string, string];

export type AutomationApplicability = {
  from: AutomationApplicabilityValue[];
  to?: AutomationApplicabilityValue[];
};

export type AutomationResponse = {
  id: number;
  slug: string;
  shortDescription: string;
  useCaseCategory?: string | null;
  tags: string[];
  engine: string;
  applicability: AutomationApplicability;
  name: string;
  featured: boolean;
  verified: boolean;
  private: boolean;
  author: string;
  amountOfUses: number;
  totalTimeSaved: number;
  openedPrs: number;
  createdAt: string;
  updatedAt: string;
  frameworks: string[];
  versions: AutomationResponseVersion[];
  /**
   * Optional fields populated by the new Registry Search API shape
   */
  pkg?: {
    id: string;
    name: string;
    displayName?: string | null;
    license?: string | null;
    repository?: string | null;
    homepage?: string | null;
    documentation?: string | null;
    bugsUrl?: string | null;
    description?: string | null;
    scope?: string | null;
    visibility?: string | null;
    access?: "public" | "private";
    downloadCount?: number;
    createdAt?: string;
    updatedAt?: string;
    latestVersion?: string | null;
    keywords?: string[];
    starCount?: number;
    author?: string | null;
  };
  owner?: {
    id: string | null;
    username: string | null;
    name: string | null;
    avatarUrl: string | null;
  };
  organization?: {
    id: string | null;
    name: string | null;
    slug: string | null;
    avatarUrl: string | null;
  } | null;
  isLegacy?: boolean;
};

export type AutomationResponseVersion = {
  id: number;
  version: string;
  shortDescription: string;
  engine: string;
  applicability?: AutomationApplicability;
  arguments: {
    kind: string;
    name: string;
    required: boolean;
  }[];
  vsCodeLink: string;
  codemodStudioExampleLink?: string;
  testProjectCommand?: string;
  sourceRepo?: string;
  amountOfUses: number;
  totalTimeSaved: number;
  openedPrs: number;
  s3Bucket: string;
  s3UploadKey: string;
  useCaseCategory?: string;
  tags: string[];
  codemodId: number;
  createdAt: string;
  updatedAt: string;
};

export type AutomationImportSchema = {
  _id: string;
  _type: "automation";
  internalTitle?: string;
  pathname: string;
  automationName: string;
  automationId: number;
  shortDescription: string;
  publishStatus: PublishStatus;
  seo: {
    title: string;
    description?: string;
  };
  useCaseCategory?: string | null;
  applicability?: AutomationApplicability;
  featured: boolean;
  verified: boolean;
  private: boolean;
  visible?: boolean;
  author: string;
  amountOfUses: number;
  totalTimeSaved: number;
  totalRuns: number;
  versions: AutomationResponseVersion[];
  openedPrs: number;
  createdAt: string;
  updatedAt: string;
  automationStories?: AutomationStories;
  filterIconDictionary?: AutomationFilterIconDictionary;
  // It's empty in response
  currentVersion?: Record<string, unknown>;
  frameworks: string[];
};

export type AutomationStories = Array<{
  title: string;
  tagline: string;
  pathname: string;
}> | null;

export type AutomationFilter = {
  id: string;
  title: string;
  values: { id: string; title: string; count: number }[];
};

export type GlobalLabels = {
  blog: {
    relatedArticles?: string;
    backToIndex?: string;
  };
  careers: {
    relatedJobs?: string;
    backToIndex?: string;
    applyToPosition?: string;
    applyToPositionDescription?: string;
    applyToPositionCTA?: string;
  };
  codemodPage: {
    ogDescription?: string;
    ctaTitle?: string;
    ctaDescription?: string;
    documentationPopup?: BlocksBody;
    documentationPopupLink: CTAData;
    runCommandPrefix?: string;
    backToIndex?: string;
    cta?: CTAData;
    runSectionTitle?: string;
    runCommandTitle?: string;
    vsCodeExtensionTitle?: string;
    vsCodeExtensionButtonLabel?: string;
    codemodStudioExampleTitle?: string;
    codemodStudioExampleButtonLabel?: string;
    textProjectTitle?: string;
    sourceRepoTitle?: string;
  };
};
export type RegistryCardData = AutomationResponse & {
  verifiedTooltip?: string;
  filterIconDictionary?: AutomationFilterIconDictionary;
};
