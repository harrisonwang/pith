'use strict';

const native = require('./index.js');

function callWithStructuredErrors(fn, args) {
  try {
    return fn(...args);
  } catch (error) {
    try {
      Object.assign(error, JSON.parse(error.message));
    } catch {
      // Preserve non-spoor errors exactly as napi-rs produced them.
    }
    throw error;
  }
}

module.exports.detectFormat = (...args) =>
  callWithStructuredErrors(native.detectFormat, args);
module.exports.parseBytes = (...args) =>
  callWithStructuredErrors(native.parseBytes, args);
module.exports.extractMedia = (...args) =>
  callWithStructuredErrors(native.extractMedia, args);
module.exports.locateQuote = (...args) =>
  callWithStructuredErrors(native.locateQuote, args);
