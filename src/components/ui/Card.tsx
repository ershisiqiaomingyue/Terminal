import React from "react";
import clsx from "clsx";

export interface CardProps extends React.HTMLAttributes<HTMLDivElement> {
  /**
   * Neutral surface level, for page layout. `surface` is the plainest white
   * panel; `muted`, `subtle` and `deep` step darker in turn. Pick a level by
   * how far back the block should sit, not by what it contains.
   */
  tone?: "surface" | "muted" | "subtle" | "deep";
  /** Structural kind, kept apart from the surface level. */
  variant?: "default" | "tool" | "interactive";
  /** Draw a hairline between header, body and footer sections. */
  dividers?: boolean;
}

const TONE_CLASS: Record<NonNullable<CardProps["tone"]>, string | undefined> = {
  surface: "card-tone-surface",
  muted: "card-tone-muted",
  subtle: "card-tone-subtle",
  deep: "card-tone-deep",
};

export const Card: React.FC<CardProps> = ({
  children,
  tone,
  variant = "default",
  dividers = false,
  className,
  ...props
}) => {
  return (
    <div
      className={clsx(
        "card",
        tone && TONE_CLASS[tone],
        variant === "tool" && "card-tool",
        variant === "interactive" && "card-interactive",
        dividers && "card-divided",
        className
      )}
      {...props}
    >
      {children}
    </div>
  );
};

export const CardHeader: React.FC<React.HTMLAttributes<HTMLDivElement>> = ({
  children,
  className,
  ...props
}) => (
  <div className={clsx("card-header", className)} {...props}>
    {children}
  </div>
);

export const CardBody: React.FC<React.HTMLAttributes<HTMLDivElement>> = ({
  children,
  className,
  ...props
}) => (
  <div className={clsx("card-body", className)} {...props}>
    {children}
  </div>
);

export const CardFooter: React.FC<React.HTMLAttributes<HTMLDivElement>> = ({
  children,
  className,
  ...props
}) => (
  <div className={clsx("card-footer", className)} {...props}>
    {children}
  </div>
);

export default Card;
