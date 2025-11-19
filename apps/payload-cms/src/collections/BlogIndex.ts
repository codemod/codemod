import { CollectionConfig } from "payload/types";
import { seoField } from "../fields/shared/seo";
import { publishStatusField } from "../fields/shared/publishStatus";

const BlogIndex: CollectionConfig = {
  slug: "blog-index",
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
      admin: {
        description:
          "Optional. Will only show in the collection's index. Filtered and paginated views won't render this title.",
      },
    },
    {
      name: "pathname",
      type: "text",
      required: true,
      unique: true,
      label: "Pathname",
      defaultValue: "/blog",
    },
    {
      name: "internalTitle",
      type: "text",
      label: "Internal title",
    },
    {
      name: "collectionTitle",
      type: "text",
      label: "Collection title",
      admin: {
        description:
          "Optional. Will only show directly above the collection's index. Filtered and paginated views won't render this title.",
      },
    },
    {
      name: "featuredPosts",
      type: "relationship",
      relationTo: "blog-articles",
      hasMany: true,
      maxRows: 2,
      minRows: 1,
      label: "Default Featured posts",
      admin: {
        description:
          "Required. These will show on the collections index when no tags are selected.",
      },
    },
    {
      name: "featuredCustomerStories",
      type: "relationship",
      relationTo: "blog-customer-stories",
      hasMany: true,
      maxRows: 2,
      minRows: 1,
      label: "Featured Customer Stories",
      admin: {
        description: "Required. These will show on blog/tag/customer-stories.",
      },
    },
    {
      name: "emptyStateText",
      type: "text",
      label: "Text for empty state",
      admin: {
        description:
          "Optional. Will only show when there are no valid entries in the collection.",
      },
    },
    {
      name: "searchPlaceholder",
      type: "text",
      required: true,
      label: "Search placeholder",
      admin: {
        description: "Search input's placeholder text. Defaults to 'Search'.",
      },
    },
    {
      name: "defaultFilterTitle",
      type: "text",
      required: true,
      label: "Default filter title",
      admin: {
        description: "Filter's default title. Defaults to 'All'.",
      },
    },
    {
      name: "cta",
      type: "relationship",
      relationTo: ["page-ctas", "page-cta-doubles", "page-cta-triples"],
      label: "Page Call to action (Optional)",
      admin: {
        description:
          "Call to action for a page. This is placed at the bottom of the page before the footer.",
      },
    },
    publishStatusField,
    seoField,
  ],
};

export default BlogIndex;
