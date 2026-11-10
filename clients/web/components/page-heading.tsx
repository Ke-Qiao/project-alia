interface PageHeadingProps {
  title: string,
  description: string
}

export function PageHeading({ title, description }: PageHeadingProps) {
  return (
    <div className="flex flex-col gap-1">
      <h1 className="font-heading font-semibold text-xl">{title}</h1>
      <p className="text-muted-foreground text-sm">{description}</p>
    </div>
  );
}
