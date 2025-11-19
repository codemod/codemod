import { CollectionConfig } from "payload/types";

const ArticleCta: CollectionConfig = {
  slug: "article-ctas",
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
      required: true,
      label: "Title",
    },
    {
      name: "subtitle",
      type: "textarea",
      label: "Subtitle",
    },
    {
      name: "cta",
      type: "group",
      required: true,
      label: "Call to action",
      fields: [
        {
          name: "label",
          type: "text",
          label: "Button label",
        },
        {
          name: "link",
          type: "text",
          required: true,
          label: "Link URL",
        },
      ],
    },
  ],
};

export default ArticleCta;
