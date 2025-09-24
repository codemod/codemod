import { defineType } from "sanity";
import { imageWithAltField } from "../shared/imageWithAltField";

export const linkedImage = defineType({
  type: "object",
  name: "linkedImage",
  title: "Linked Image",
  fields: [
    { ...imageWithAltField, name: "image" },
    {
      name: "link",
      title: "Link",
      type: "link",
      description: "Wrap the image with this link",
    },
    { type: "string", name: "caption", title: "Caption" },
  ],
  preview: {
    select: {
      title: "caption",
      media: "image.lightImage",
      href: "link.href",
    },
    prepare({ title, media, href }) {
      return {
        title: title || href || "Linked Image",
        media,
      };
    },
  },
});
