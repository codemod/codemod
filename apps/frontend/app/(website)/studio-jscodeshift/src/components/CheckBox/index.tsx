import type { ChangeEvent } from "react";
import Text from "../Text";

type CheckboxProps = {
  checked?: boolean;
  label: string;
  value?: string;
  onChange: (e: ChangeEvent<HTMLInputElement>) => void;
  className?: string;
  tooltipId?: string;
  tooltipContent?: string;
};

const Checkbox = ({
  onChange,
  checked,
  label,
  value,
  className,
  tooltipId,
  tooltipContent,
}: CheckboxProps) => (
  <div className="flex">
    <label
      className={`flex items-center ${className}`}
      data-tooltip-content={tooltipContent}
      data-tooltip-id={tooltipId}
    >
      <input
        checked={checked}
        className="h-4 w-4 rounded border-border-light bg-gray-100 text-blue-600 focus:ring-2 focus:ring-blue-500 dark:border-border-dark dark:bg-gray-700 dark:ring-offset-gray-800 dark:focus:ring-blue-600"
        onChange={onChange}
        type="checkbox"
        value={value}
      />
      <Text className="ml-2" heading="span">
        {label}
      </Text>
    </label>
  </div>
);

export default Checkbox;
