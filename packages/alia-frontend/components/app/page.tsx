import { FlashList, type FlashListProps, type ListRenderItemInfo } from '@shopify/flash-list';
import * as React from 'react';
import { Text } from '@/components/ui/text';
import { cn } from '@/lib/utils';
import { ScrollView, StyleSheet, useWindowDimensions, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

type PageProps = {
  title: string;
  description?: string;
  actions?: React.ReactNode;
  children: React.ReactNode;
  className?: string;
};

export function Page({ title, description, actions, children, className }: PageProps) {
  const { width } = useWindowDimensions();
  const insets = useSafeAreaInsets();
  const desktop = width >= 900;
  const bottomPadding = (desktop ? 32 : 96) + insets.bottom;

  return (
    <ScrollView
      className="flex-1"
      contentContainerClassName="p-4 md:p-6"
      contentContainerStyle={{ paddingBottom: bottomPadding }}>
      <View className={cn('mx-auto w-full max-w-6xl gap-5', className)}>
        <PageHeading title={title} description={description} actions={actions} />
        {children}
      </View>
    </ScrollView>
  );
}

type ListPageProps<T> = Omit<PageProps, 'children'> & {
  data: ReadonlyArray<T>;
  renderItem: (info: ListRenderItemInfo<T>) => React.ReactElement | null;
  keyExtractor: NonNullable<FlashListProps<T>['keyExtractor']>;
  empty?: React.ReactNode;
  header?: React.ReactNode;
  footer?: React.ReactNode;
  extraData?: unknown;
};

export function ListPage<T>({
  actions,
  className,
  data,
  description,
  empty,
  extraData,
  footer,
  header,
  keyExtractor,
  renderItem,
  title,
}: ListPageProps<T>) {
  const { width } = useWindowDimensions();
  const insets = useSafeAreaInsets();
  const desktop = width >= 900;
  const horizontalPadding = width >= 768 ? 24 : 16;
  const bottomPadding = (desktop ? 32 : 96) + insets.bottom;

  return (
    <FlashList
      data={data}
      extraData={extraData}
      keyExtractor={keyExtractor}
      renderItem={(info) => <View style={styles.frame}>{renderItem(info)}</View>}
      ItemSeparatorComponent={ListSeparator}
      ListHeaderComponent={
        <View style={styles.frame} className={cn('gap-5', className)}>
          <PageHeading title={title} description={description} actions={actions} />
          {header}
        </View>
      }
      ListHeaderComponentStyle={styles.header}
      ListEmptyComponent={empty ? <View style={styles.frame}>{empty}</View> : null}
      ListFooterComponent={footer ? <View style={styles.frame}>{footer}</View> : null}
      ListFooterComponentStyle={styles.footer}
      contentContainerStyle={{
        paddingBottom: bottomPadding,
        paddingHorizontal: horizontalPadding,
        paddingTop: horizontalPadding,
      }}
      showsVerticalScrollIndicator={false}
      style={styles.list}
    />
  );
}

function PageHeading({
  actions,
  description,
  title,
}: {
  title: string;
  description?: string;
  actions?: React.ReactNode;
}) {
  return (
    <View className="gap-3 sm:flex-row sm:items-start sm:justify-between">
      <View className="max-w-3xl gap-1">
        <Text className="text-2xl font-semibold tracking-normal">{title}</Text>
        {description ? <Text className="text-muted-foreground text-sm leading-5">{description}</Text> : null}
      </View>
      {actions ? <View className="flex-row flex-wrap items-center gap-2">{actions}</View> : null}
    </View>
  );
}

function ListSeparator() {
  return <View className="h-3" />;
}

const styles = StyleSheet.create({
  list: {
    flex: 1,
  },
  frame: {
    alignSelf: 'center',
    maxWidth: 1152,
    width: '100%',
  },
  header: {
    marginBottom: 12,
  },
  footer: {
    marginTop: 16,
  },
});
