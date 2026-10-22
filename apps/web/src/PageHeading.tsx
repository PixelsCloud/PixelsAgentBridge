import type { ReactNode } from 'react';
import { Typography } from 'antd';

/** Shared page hierarchy, including room for page-specific actions. */
export function PageHeading({ title, description, actions }: {
  title: string;
  description?: string;
  actions?: ReactNode;
}) {
  return <div className="page-heading">
    <div>
      <Typography.Title level={2}>{title}</Typography.Title>
      {description && <Typography.Paragraph type="secondary">{description}</Typography.Paragraph>}
    </div>
    {actions && <div className="page-heading-actions">{actions}</div>}
  </div>;
}
