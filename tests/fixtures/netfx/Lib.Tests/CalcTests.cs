using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace Lib.Tests
{
    [TestClass]
    public class CalcTests
    {
        [TestMethod]
        public void AddsTwoNumbers()
        {
            Assert.AreEqual(5, Lib.Calc.Add(2, 3));
        }
    }
}
