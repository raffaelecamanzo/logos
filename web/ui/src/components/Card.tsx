/*
 * Card (S-193, FR-UI-23; frontend-design §5). The workhorse surface: a raised
 * panel with the NON-REMOVABLE 3px signal-red top accent edge and the 4px brand
 * radius — the brand's card grammar, preserved in both themes. An optional `title`
 * renders as the ≤8-word <h3>.
 */

import type { ReactNode } from "react";

import styles from "./Card.module.css";

export interface CardProps {
  /** Optional card title — rendered as an <h3> (the brand's card-title role). */
  title?: ReactNode;
  /** Optional content rendered on the title row, right-aligned (badge, action). */
  aside?: ReactNode;
  children: ReactNode;
  className?: string;
}

export function Card({ title, aside, children, className }: CardProps) {
  const cls = [styles.card, className].filter(Boolean).join(" ");
  return (
    <section className={cls}>
      {(title || aside) && (
        <div className={styles.head}>
          {title && <h3 className={styles.title}>{title}</h3>}
          {aside && <div className={styles.aside}>{aside}</div>}
        </div>
      )}
      {children}
    </section>
  );
}
