// Every upload counts against the project's daily limit of evaluations, and a few runs
// in a day would reach it: each run starts from zero.
const module_ = require('./module');

module.exports = async () => {
    module_.forgetEvaluations();
};
