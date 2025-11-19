import { CollectionConfig } from "payload/types";

const PageCtaTriple: CollectionConfig = {
  slug: "page-cta-triples",
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
      name: "splitPattern",
      type: "text",
      label: "Split pattern",
      admin: {
        description:
          'Creates a line break in the title. Input the characters that should preceed the line break. E.g. "."',
      },
    },
    {
      name: "paragraph",
      type: "richText",
      required: true,
      label: "Paragraph",
    },
    {
      name: "ctas",
      type: "array",
      minRows: 3,
      maxRows: 3,
      required: true,
      label: "Call to actions",
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

export default PageCtaTriple;
