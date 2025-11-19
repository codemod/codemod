import { CollectionConfig } from "payload/types";
import { linkField } from "../fields/shared/link";

const PageCta: CollectionConfig = {
  slug: "page-ctas",
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
      name: "paragraph",
      type: "richText",
      required: true,
      label: "Paragraph",
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

export default PageCta;
