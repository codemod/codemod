import CustomPTEditor from "@/sanity/lib/components/CustomPTEditor";
import { getCustomBody } from "../helpers/getCustomBody";
import { codeSnippet } from "./codeSnippet";
import { collapsible } from "./collapsible";
import { imageBlock } from "./image";
import { linkedImage } from "./linkedImage";
import { muxVideoWithCaption } from "./muxVideoWithCaption";
import { quoteBlock } from "./quoteBlock";
import { ptTable } from "./table";
import { twitterEmbed } from "./twitterEmbed";
import { youtubeVideo } from "./youtubeVideo";

const ptBlocks = [
  muxVideoWithCaption,
  youtubeVideo,
  imageBlock,
  linkedImage,
  twitterEmbed,
  codeSnippet,
  quoteBlock,
  ptTable,
];

export const ptBody = {
  name: "ptBody",
  title: "Rich Text with blocks",
  ...getCustomBody({
    styles: true,
    lists: true,
    blockTypes: [...ptBlocks, collapsible].map((block) => block.name),
  }),
  components: {
    input: CustomPTEditor,
  },
};

export const ptBodyCollapsible = {
  name: "ptBodyCollapsible",
  title: "Rich Text with blocks without collapsible",
  ...getCustomBody({
    styles: true,
    lists: true,
    blockTypes: ptBlocks.map((block) => block.name),
  }),
  components: {
    input: CustomPTEditor,
  },
};
