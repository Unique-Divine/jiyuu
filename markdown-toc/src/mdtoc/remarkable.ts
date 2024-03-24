import { Remarkable } from "remarkable"

export interface IRemarkable extends RemarkableComponents {
  // constructor(preset?: string, options?: RemarkableOptions): IRemarkable;

  /**
   * Set options as an alternative to passing them
   * to the constructor.
   *
   * ```js
   * md.set({typographer: true});
   * ```
   * @param {Object} `options`
   * @api public
   */
  set(options: RemarkableOptions): void

  /**
   * Batch loader for components rules states, and options
   *
   * @param  {Object} `presets`
   */
  configure(presets: RemarkablePresets): void

  /**
   * Use a plugin.
   *
   * ```js
   * var md = new Remarkable();
   *
   * md.use(plugin1)
   *   .use(plugin2, opts)
   *   .use(plugin3);
   * ```
   *
   * @param  {Function} `plugin`
   * @param  {Object} `options`
   * @return {Object} `Remarkable` for chaining
   */
  use(plugin: Function, options?: RemarkableOptions): IRemarkable

  /**
   * Parse the input `string` and return a tokens array.
   * Modifies `env` with definitions data.
   *
   * @param  {String} `str`
   * @param  {Object} `env`
   * @return {Array} Array of tokens
   */
  parse(str: string, env?: RemarkableEnv): any[]

  /**
   * The main `.render()` method that does all the magic :)
   *
   * @param  {String} `str`
   * @param  {Object} `env`
   * @return {string} Rendered HTML.
   */
  render(str: string, env?: RemarkableEnv): string

  /**
   * Parse the given content `string` as a single string.
   *
   * @param  {String} `str`
   * @param  {Object} `env`
   * @return {Array} Array of tokens
   */
  parseInline(str: string, env?: RemarkableEnv): any[]

  /**
   * Render a single content `string`, without wrapping it
   * to paragraphs
   *
   * @param  {String} `str`
   * @param  {Object} `env`
   * @return {String}
   */
  renderInline(str: string, env?: RemarkableEnv): string

  renderer: any
  ruler: any
  options: RemarkableOptions
}

interface RemarkableOptions {
  html?: boolean
  xhtmlOut?: boolean
  breaks?: boolean
  langPrefix?: string
  linkTarget?: string
  typographer?: boolean
  quotes?: string
  highlight?: (str: string, lang: string) => string
  maxNesting?: number
}

interface RemarkableComponents {
  core: any
  block: any
  inline: any
}

type RemarkableEnv = Object

interface RemarkablePresets {
  default: any
  full: any
  commonmark: any
}

export class RemarkablePlus implements IRemarkable, RemarkableComponents {
  core: Remarkable

  constructor({
    preset,
    options,
  }: {
    preset?: string
    options?: RemarkableOptions
  }) {
    const core = new Remarkable(preset ?? "default", options)
    this.core = core
  }

  set = (options: RemarkableOptions): void => {
    this.core.set(options)
  }

  configure = (presets: RemarkablePresets): void => {
    this.core.configure(presets)
  }

  use = (plugin: Function, options?: RemarkableOptions): IRemarkable => {
    this.core.use(plugin, options)
    return this // Return `this` to allow for chaining.
  }

  parse = (str: string, env?: RemarkableEnv): any[] => this.core.parse(str, env)

  render = (str: string, env?: RemarkableEnv): string =>
    this.core.render(str, env)

  parseInline = (str: string, env?: RemarkableEnv): any[] =>
    this.core.parseInline(str, env)

  renderInline = (str: string, env?: RemarkableEnv): string =>
    this.core.renderInline(str, env)

  get inline(): any {
    return this.core.inline
  }

  get block(): any {
    return this.core.block
  }

  get renderer(): any {
    return this.core.renderer
  }

  get ruler(): any {
    return this.core.ruler
  }

  get options(): RemarkableOptions {
    return this.core.options
  }
}
