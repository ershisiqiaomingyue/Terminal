import React from "react";
import clsx from "clsx";

export interface TableProps extends React.TableHTMLAttributes<HTMLTableElement> {
  containerClassName?: string;
}

export const Table: React.FC<TableProps> = ({
  children,
  className,
  containerClassName,
  ...props
}) => {
  return (
    <div className={clsx("table-container", containerClassName)}>
      <table className={clsx("table", className)} {...props}>
        {children}
      </table>
    </div>
  );
};

export default Table;
