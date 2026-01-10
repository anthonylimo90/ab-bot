'use client';

import * as React from 'react';
import {
  Controller,
  ControllerProps,
  FieldPath,
  FieldValues,
  FormProvider,
  useFormContext,
} from 'react-hook-form';
import { cn } from '@/lib/utils';
import { Label } from '@/components/ui/label';
import { Input } from '@/components/ui/input';

// Re-export FormProvider for convenience
export { FormProvider };

// Form context for field state
const FormFieldContext = React.createContext<{
  name: string;
  id: string;
}>({ name: '', id: '' });

// FormField wrapper that provides context
export function FormField<
  TFieldValues extends FieldValues = FieldValues,
  TName extends FieldPath<TFieldValues> = FieldPath<TFieldValues>,
>({ ...props }: ControllerProps<TFieldValues, TName>) {
  const id = React.useId();
  return (
    <FormFieldContext.Provider value={{ name: props.name, id }}>
      <Controller {...props} />
    </FormFieldContext.Provider>
  );
}

// Hook to access form field context
export function useFormField() {
  const fieldContext = React.useContext(FormFieldContext);
  const { getFieldState, formState } = useFormContext();

  if (!fieldContext.name) {
    throw new Error('useFormField must be used within FormField');
  }

  const fieldState = getFieldState(fieldContext.name, formState);

  return {
    id: fieldContext.id,
    name: fieldContext.name,
    ...fieldState,
  };
}

// Form item wrapper
interface FormItemProps extends React.HTMLAttributes<HTMLDivElement> {}

export const FormItem = React.forwardRef<HTMLDivElement, FormItemProps>(
  ({ className, ...props }, ref) => {
    return (
      <div ref={ref} className={cn('space-y-2', className)} {...props} />
    );
  }
);
FormItem.displayName = 'FormItem';

// Form label with error state
interface FormLabelProps extends React.ComponentPropsWithoutRef<typeof Label> {}

export const FormLabel = React.forwardRef<HTMLLabelElement, FormLabelProps>(
  ({ className, ...props }, ref) => {
    const { id, error } = useFormField();
    return (
      <Label
        ref={ref}
        htmlFor={id}
        error={!!error}
        className={className}
        {...props}
      />
    );
  }
);
FormLabel.displayName = 'FormLabel';

// Form control wrapper for inputs
interface FormControlProps extends React.HTMLAttributes<HTMLDivElement> {}

export const FormControl = React.forwardRef<HTMLDivElement, FormControlProps>(
  ({ ...props }, ref) => {
    const { id, error } = useFormField();
    return (
      <div
        ref={ref}
        id={id}
        aria-invalid={!!error}
        aria-describedby={error ? `${id}-error` : undefined}
        {...props}
      />
    );
  }
);
FormControl.displayName = 'FormControl';

// Form description/helper text
interface FormDescriptionProps extends React.HTMLAttributes<HTMLParagraphElement> {}

export const FormDescription = React.forwardRef<HTMLParagraphElement, FormDescriptionProps>(
  ({ className, ...props }, ref) => {
    return (
      <p
        ref={ref}
        className={cn('text-sm text-muted-foreground', className)}
        {...props}
      />
    );
  }
);
FormDescription.displayName = 'FormDescription';

// Form error message
interface FormMessageProps extends React.HTMLAttributes<HTMLParagraphElement> {}

export const FormMessage = React.forwardRef<HTMLParagraphElement, FormMessageProps>(
  ({ className, children, ...props }, ref) => {
    const { id, error } = useFormField();
    const message = error?.message || children;

    if (!message) return null;

    return (
      <p
        ref={ref}
        id={`${id}-error`}
        role="alert"
        className={cn('text-sm font-medium text-destructive', className)}
        {...props}
      >
        {message}
      </p>
    );
  }
);
FormMessage.displayName = 'FormMessage';

// Convenience component: Input with label and error handling
interface FormInputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  label: string;
  description?: string;
  name: string;
}

export function FormInput({ label, description, name, className, ...inputProps }: FormInputProps) {
  const { register, formState: { errors } } = useFormContext();
  const error = errors[name];

  return (
    <div className="space-y-2">
      <Label htmlFor={name} error={!!error}>
        {label}
      </Label>
      <Input
        id={name}
        error={!!error}
        {...register(name, { valueAsNumber: inputProps.type === 'number' })}
        {...inputProps}
        className={className}
      />
      {description && !error && (
        <p className="text-sm text-muted-foreground">{description}</p>
      )}
      {error && (
        <p className="text-sm font-medium text-destructive" role="alert">
          {error.message as string}
        </p>
      )}
    </div>
  );
}
