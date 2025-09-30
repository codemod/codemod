import type { ReactNode, SelectHTMLAttributes } from "react";

type Props = Omit<SelectHTMLAttributes<HTMLSelectElement>, "className"> & {
  children?: ReactNode;
  className?: string;
};

const Select = ({ children, className, ...restProps }: Props) => (
  <select
    className={`block rounded-lg border border-border-light bg-gray-50 p-2.5 py-1 text-sm text-gray-900  focus:border-blue-500 focus:ring-blue-500 dark:border-border-dark dark:bg-gray-700 dark:text-white dark:placeholder-gray-400 dark:focus:border-blue-500 dark:focus:ring-blue-500 ${
      className ?? "w-full"
    } `}
    {...restProps}
  >
    {children}
  </select>
);

export default Select;
