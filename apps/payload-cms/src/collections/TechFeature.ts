import { CollectionConfig } from "payload/types";
import { imageWithAltField } from "../fields/shared/imageWithAlt";

const TechFeature: CollectionConfig = {
  slug: "tech-features",
  admin: {
    useAsTitle: "title",
  },
  access: {
    read: () => true,
  },
  versions: {
    drafts: true,
  },
  fields: [
    {
      name: "title",
      type: "text",
      label: "Title",
    },
    {
      ...imageWithAltField,
      name: "logo",
      label: "Logo",
    },
    {
      name: "url",
      type: "text",
      label: "URL",
    },
  ],
};

export default TechFeature;
