// Documents
import about from "./documents/about";
import articleCta from "./documents/articleCta";
import { blogArticle } from "./documents/blogArticle";
import { blogAuthor } from "./documents/blogAuthor";
import { blogCustomerStory } from "./documents/blogCustomerStory";
import { blogIndex } from "./documents/blogIndex";
import { blogTag } from "./documents/blogTag";
import { careers } from "./documents/careers";
import contact from "./documents/contact";
import { filterIconDictionary } from "./documents/filterIconDictionary";
import footer from "./documents/footer";
import { globalLabels } from "./documents/globalLabels";
import { job } from "./documents/job";
import navigation from "./documents/navigation";
import { notFound } from "./documents/notFound";
import page from "./documents/page";
import pageCta from "./documents/pageCta";
import pageCtaDouble from "./documents/pageCtaDouble";
import pageCtaTriple from "./documents/pageCtaTriple";
import pricing from "./documents/pricing";
import { registryIndex } from "./documents/registryIndex";
import { settings } from "./documents/settings";
import techFeature from "./documents/techFeature";
import { textPage } from "./documents/textPage";

// Objects
import { admonition } from "./objects/admonition";
import { automationVersion } from "./objects/automationVersion";
// Sections
import { codeSnippet } from "./objects/codeSnippet";
import { collapsible } from "./objects/collapsible";
import { cta } from "./objects/cta";
import { imageBlock } from "./objects/image";
import { linkedImage } from "./objects/linkedImage";
import { link } from "./objects/link";
import { muxVideo } from "./objects/muxVideo";
import { muxVideoWithCaption } from "./objects/muxVideoWithCaption";
import { ogImage } from "./objects/ogImage";
import { ptBody, ptBodyCollapsible } from "./objects/ptBody";
import { quoteBlock } from "./objects/quoteBlock";
import { richText } from "./objects/richText";
import { sectionHero } from "./objects/sectionHero";
import { sectionsBody } from "./objects/sectionsBody";
import { seo } from "./objects/seo";
import { stats } from "./objects/stats";
import { styledCta } from "./objects/styledCta";
import { ptTable } from "./objects/table";
import { tag } from "./objects/tag";
import { twitterEmbed } from "./objects/twitterEmbed";
import { youtubeVideo } from "./objects/youtubeVideo";
import sections from "./sections";
//Shared Objects
import { publishStatusField } from "./shared/publishStatusField";

const schemas = [
  // Documents
  page,
  pricing,
  navigation,
  footer,
  settings,
  pageCta,
  pageCtaDouble,
  pageCtaTriple,
  articleCta,
  contact,
  about,
  textPage,
  blogArticle,
  blogCustomerStory,
  blogAuthor,
  blogTag,
  blogIndex,
  notFound,
  careers,
  job,
  techFeature,
  registryIndex,
  globalLabels,
  filterIconDictionary,
  // Objects
  collapsible,
  sectionsBody,
  imageBlock,
  linkedImage,
  codeSnippet,
  twitterEmbed,
  ptBody,
  ptBodyCollapsible,
  richText,
  tag,
  ogImage,
  link,
  cta,
  styledCta,
  seo,
  publishStatusField,
  stats,
  quoteBlock,
  muxVideo,
  muxVideoWithCaption,
  youtubeVideo,
  automationVersion,
  ptTable,
  admonition,
  //   Sections
  ...sections,
  sectionHero,
];

export default schemas;
