import { ReactNode, useEffect, useId, useRef, useState } from "react";
import { Check, ChevronDown, Search } from "lucide-react";

export interface CustomSelectOption<T extends string> {
    value: T;
    label: string;
    group?: string;
    description?: string;
    badge?: string;
    disabled?: boolean;
    icon?: ReactNode;
}

interface CustomSelectProps<T extends string> {
    label: string;
    name?: string;
    value: T;
    options: CustomSelectOption<T>[];
    onChange: (value: T) => void;
    className?: string;
}

export function CustomSelect<T extends string>({
    label,
    name,
    value,
    options,
    onChange,
    className = "",
}: CustomSelectProps<T>) {
    const [open, setOpen] = useState(false);
    const [query, setQuery] = useState("");
    const rootRef = useRef<HTMLDivElement>(null);
    const labelId = useId();
    const selected =
        options.find((option) => option.value === value) ?? options[0];
    const groups = Array.from(
        new Set(options.map((option) => option.group ?? "Options")),
    );
    const visibleOptions = options.filter((option) => {
        const search = query.trim().toLocaleLowerCase();
        return (
            !search ||
            option.label.toLocaleLowerCase().includes(search) ||
            option.description?.toLocaleLowerCase().includes(search) ||
            option.group?.toLocaleLowerCase().includes(search)
        );
    });

    useEffect(() => {
        if (!open) return;
        const close = (event: PointerEvent) => {
            if (
                rootRef.current &&
                !rootRef.current.contains(event.target as Node)
            ) {
                setOpen(false);
            }
        };
        const closeOnEscape = (event: KeyboardEvent) => {
            if (event.key === "Escape") setOpen(false);
        };
        document.addEventListener("pointerdown", close);
        document.addEventListener("keydown", closeOnEscape);
        return () => {
            document.removeEventListener("pointerdown", close);
            document.removeEventListener("keydown", closeOnEscape);
        };
    }, [open]);

    useEffect(() => {
        if (!open) setQuery("");
    }, [open]);

    return (
        <div className={`custom-select-field ${className}`} ref={rootRef}>
            <span className="custom-select-label" id={labelId}>
                {label}
            </span>
            {name && <input type="hidden" name={name} value={value} />}
            <button
                className="custom-select-trigger"
                type="button"
                role="combobox"
                aria-labelledby={labelId}
                aria-expanded={open}
                aria-haspopup="listbox"
                onClick={() => setOpen((current) => !current)}
            >
                <span className="custom-select-icon">{selected?.icon}</span>
                <span className="custom-select-value">
                    <strong>{selected?.label}</strong>
                    {selected?.description && (
                        <small>{selected.description}</small>
                    )}
                </span>
                <ChevronDown size={16} />
            </button>
            {open && (
                <div
                    className="custom-select-menu"
                    role="listbox"
                    aria-labelledby={labelId}
                >
                    {options.length > 12 && (
                        <label className="custom-select-search">
                            <Search size={15} />
                            <input
                                autoFocus
                                value={query}
                                onChange={(event) =>
                                    setQuery(event.target.value)
                                }
                                placeholder={`Search ${label.toLocaleLowerCase()}`}
                            />
                        </label>
                    )}
                    {groups
                        .filter((group) =>
                            visibleOptions.some(
                                (option) =>
                                    (option.group ?? "Options") === group,
                            ),
                        )
                        .map((group) => (
                            <section
                                className="custom-select-group"
                                key={group}
                            >
                                <p>{group}</p>
                                {visibleOptions
                                    .filter(
                                        (option) =>
                                            (option.group ?? "Options") ===
                                            group,
                                    )
                                    .map((option) => (
                                        <button
                                            className={`custom-select-option ${option.value === value ? "selected" : ""}`}
                                            type="button"
                                            role="option"
                                            aria-selected={
                                                option.value === value
                                            }
                                            aria-disabled={option.disabled}
                                            disabled={option.disabled}
                                            key={option.value}
                                            onClick={() => {
                                                onChange(option.value);
                                                setOpen(false);
                                            }}
                                        >
                                            <span className="custom-select-icon">
                                                {option.icon}
                                            </span>
                                            <span className="custom-select-value">
                                                <strong>{option.label}</strong>
                                                {option.description && (
                                                    <small>
                                                        {option.description}
                                                    </small>
                                                )}
                                            </span>
                                            {option.badge && (
                                                <span className="custom-select-badge">
                                                    {option.badge}
                                                </span>
                                            )}
                                            {option.value === value &&
                                                !option.disabled && (
                                                    <Check size={16} />
                                                )}
                                        </button>
                                    ))}
                            </section>
                        ))}
                </div>
            )}
        </div>
    );
}
