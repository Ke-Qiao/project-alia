import { cn } from '@/lib/utils';
import * as SwitchPrimitives from '@rn-primitives/switch';
import { Platform, StyleSheet } from 'react-native';
import Animated, {
  useAnimatedStyle,
  useDerivedValue,
  withSpring,
  withTiming,
} from 'react-native-reanimated';

type SwitchProps = React.ComponentProps<typeof SwitchPrimitives.Root>;

const Switch = Platform.select({
  web: WebSwitch,
  native: NativeSwitch,
  default: WebSwitch,
});

function WebSwitch({ className, ...props }: SwitchProps) {
  return (
    <SwitchPrimitives.Root
      className={cn(
        'flex h-[1.15rem] w-8 shrink-0 flex-row items-center rounded-full border border-transparent shadow-sm shadow-black/5',
        'focus-visible:border-ring focus-visible:ring-ring/50 peer inline-flex transition-all outline-none focus-visible:ring-[3px] disabled:cursor-not-allowed',
        props.checked ? 'bg-primary' : 'bg-input dark:bg-input/80',
        props.disabled && 'opacity-50',
        className
      )}
      {...props}>
      <SwitchPrimitives.Thumb
        className={cn(
          'bg-background size-4 rounded-full transition-transform',
          'pointer-events-none block ring-0',
          props.checked
            ? 'dark:bg-primary-foreground translate-x-3.5'
            : 'dark:bg-foreground translate-x-0'
        )}
      />
    </SwitchPrimitives.Root>
  );
}

function NativeSwitch({ className, ...props }: SwitchProps) {
  const checked = props.checked;
  const disabled = props.disabled;
  const activeOpacity = useDerivedValue(() => withTiming(checked ? 1 : 0, { duration: 160 }));
  const thumbX = useDerivedValue(() =>
    withSpring(checked ? 14 : 0, {
      damping: 18,
      mass: 0.45,
      stiffness: 280,
      overshootClamping: true,
    })
  );

  const activeTrackStyle = useAnimatedStyle(() => ({
    opacity: activeOpacity.value,
  }));

  const thumbStyle = useAnimatedStyle(() => ({
    transform: [{ translateX: thumbX.value }],
  }));

  return (
    <SwitchPrimitives.Root
      className={cn(
        'bg-input dark:bg-input/80 relative flex h-[1.15rem] w-8 shrink-0 flex-row items-center overflow-hidden rounded-full border border-transparent shadow-sm shadow-black/5',
        disabled && 'opacity-50',
        className
      )}
      {...props}>
      <Animated.View
        pointerEvents="none"
        className="bg-primary rounded-full"
        style={[StyleSheet.absoluteFillObject, activeTrackStyle]}
      />
      <SwitchPrimitives.Thumb asChild>
        <Animated.View
          className={cn(
            'bg-background size-4 rounded-full shadow-sm shadow-black/10',
            checked ? 'dark:bg-primary-foreground' : 'dark:bg-foreground'
          )}
          style={thumbStyle}
        />
      </SwitchPrimitives.Thumb>
    </SwitchPrimitives.Root>
  );
}

export { Switch };
