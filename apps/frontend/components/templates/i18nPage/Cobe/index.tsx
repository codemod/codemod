import { Globe } from "./Globe";
import { NumberTicker } from "./NumberTicker";
import TextTransitions from "./TextTransitions";

const CobeSection = () => {
  return (
    <div className="px-6 lg:px-[80px] relative w-full py-[80px]">
      <div className="grid items-center gap-4 md:grid-cols-2">
        <div className="space-y-5 text-black dark:text-white">
          <TextTransitions />

          <p className="mb-2 body-l uppercase opacity-50">Revenue Growth</p>
          <p className="whitespace-pre-wrap s-heading tracking-tighter text-black md:text-4xl dark:text-white">
            <NumberTicker value={5} suffix="x" />
          </p>
          <p className="body-s-medium">More revenue potential</p>
          <p className="body-l max-w-2xl">
            Enter the global market faster and drive unprecedented business
            growth, all while freeing your engineering team from thousands of
            tedious code changes. Let engineers focus on building core product
            features, not preparing the app for internationalization.
          </p>
        </div>
        <Globe className="" />
      </div>
    </div>
  );
};

export default CobeSection;
