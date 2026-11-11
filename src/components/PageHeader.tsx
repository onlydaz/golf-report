import type { ReactNode } from "react";

interface PageHeaderProps {
  eyebrow: string;
  title: string;
  description: string;
  extra?: ReactNode;
}

function PageHeader({
  eyebrow,
  title,
  description,
  extra,
}: PageHeaderProps) {
  return (
    <div className="page-heading">
      <div>
        <div className="eyebrow">
          {eyebrow}
        </div>

        <h1 className="page-title">
          {title}
        </h1>

        <p className="page-description">
          {description}
        </p>
      </div>

      {extra}
    </div>
  );
}

export default PageHeader;
