import {
  LazyMotion,
  domMax,
  MotionConfig,
  m,
  useReducedMotion,
} from "motion/react";

export function MotionShell({ children }) {
  return (
    <LazyMotion features={domMax} strict>
      <MotionConfig reducedMotion="user">{children}</MotionConfig>
    </LazyMotion>
  );
}

export function NavHighlight({ page }) {
  const reduced = useReducedMotion();
  return (
    <m.span
      aria-hidden="true"
      className="nav-highlight"
      layoutId="workspace-selection"
      layoutDependency={page}
      initial={false}
      transition={
        reduced
          ? { duration: 0 }
          : { type: "spring", stiffness: 480, damping: 40, mass: 0.8 }
      }
    />
  );
}

export const dialogTransition = {
  transition: {
    in: { opacity: 1, transform: "none" },
    out: { opacity: 0, transform: "translateY(6px)" },
    common: { transformOrigin: "center" },
    transitionProperty: "transform, opacity",
  },
  duration: 180,
  exitDuration: 120,
  timingFunction: "cubic-bezier(0.2, 0.8, 0.2, 1)",
};
